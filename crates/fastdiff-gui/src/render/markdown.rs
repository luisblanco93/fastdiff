//! Rendered Markdown preview for full-file documents.
//!
//! The text comes from rows the GUI already received, so no helper protocol change is needed and
//! nothing here reads files or the network. Images degrade because no file or HTTP loader exists.

use crate::worker::PreparedDocument;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::sync::{Arc, Weak};

/// Largest document rendered as Markdown. This is an unmeasured bound: `CommonMarkViewer::show`
/// parses and lays out the whole document every frame, so bigger files fall back to source.
pub(crate) const MARKDOWN_RENDER_LIMIT_BYTES: usize = 512 * 1024;

pub(crate) const MARKDOWN_LIMIT_NOTICE: &str =
    "Markdown preview is limited to 512 KiB; showing source.";

/// Space kept between the window edge and the reading column.
pub(crate) const PREVIEW_SIDE_MARGIN: f32 = 24.0;
/// Widest the reading column grows; wider windows center it.
const PREVIEW_MAX_COLUMN_WIDTH: f32 = 1024.0;
/// Narrowest the column gets before it simply fills the remaining width.
const PREVIEW_MIN_COLUMN_WIDTH: f32 = 120.0;
/// Space above and below the rendered content.
const PREVIEW_VERTICAL_MARGIN: f32 = 14.0;
/// Extra space between blocks over egui's default. A paragraph is a run of separate labels, so
/// very large values would space wrapped lines unevenly.
const PREVIEW_BLOCK_SPACING: f32 = 8.0;
/// Body text is this much larger than the source font size.
const PREVIEW_BODY_OFFSET: f32 = 2.0;
/// Monospace (inline code and code blocks) relative to the body size.
const PREVIEW_MONO_RATIO: f32 = 0.9;
/// Level 1 headings relative to the body size. egui_commonmark interpolates H2 to H6 between the
/// body and this size, so a larger ratio makes the lower levels too big.
const PREVIEW_HEADING_RATIO: f32 = 1.7;

/// URL schemes a rendered link may hand to the system; everything else is dropped.
const OPENABLE_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

pub(crate) fn is_markdown_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

pub(crate) fn is_openable_url(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, rest)| {
        !rest.is_empty()
            && OPENABLE_SCHEMES
                .iter()
                .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
    })
}

pub(crate) fn fits_render_limit(bytes: usize) -> bool {
    bytes <= MARKDOWN_RENDER_LIMIT_BYTES
}

/// Returns `(space left of the column, column width)` for a viewport of `available` pixels.
fn column_layout(available: f32) -> (f32, f32) {
    let available = available.max(0.0);
    let width = (available - 2.0 * PREVIEW_SIDE_MARGIN)
        .clamp(PREVIEW_MIN_COLUMN_WIDTH, PREVIEW_MAX_COLUMN_WIDTH)
        .min(available);
    (((available - width) / 2.0).max(0.0), width)
}

/// Returns `(body, monospace, heading)` text sizes derived from the source font size.
fn preview_sizes(font_size: f32) -> (f32, f32, f32) {
    let body = font_size + PREVIEW_BODY_OFFSET;
    (
        body,
        body * PREVIEW_MONO_RATIO,
        body * PREVIEW_HEADING_RATIO,
    )
}

/// Scales the preview text styles in `style`. Body and Heading switch to `family` when given
/// (H2 to H6 use Body); Monospace keeps its family so code stays monospaced.
fn apply_preview_style(style: &mut egui::Style, font_size: f32, family: Option<egui::FontFamily>) {
    let (body, mono, heading) = preview_sizes(font_size);
    for (text_style, size) in [
        (egui::TextStyle::Body, body),
        (egui::TextStyle::Monospace, mono),
        (egui::TextStyle::Heading, heading),
    ] {
        if let Some(font) = style.text_styles.get_mut(&text_style) {
            font.size = size;
            if let Some(family) = family
                .as_ref()
                .filter(|_| text_style != egui::TextStyle::Monospace)
            {
                font.family = family.clone();
            }
        }
    }
    style.spacing.item_spacing.y += PREVIEW_BLOCK_SPACING;
}

/// Drops `OpenUrl` commands at or after `first_new` whose URL is not openable.
fn drop_unsafe_links(commands: &mut Vec<egui::OutputCommand>, first_new: usize) {
    let mut index = 0;
    commands.retain(|command| {
        let keep = index < first_new
            || !matches!(command, egui::OutputCommand::OpenUrl(open) if !is_openable_url(&open.url));
        index += 1;
        keep
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkdownDisplay {
    Rendered,
    Source,
    /// Rendered was requested but the document exceeds the size limit.
    TooLarge,
}

/// Per-tab preview state. The text exists only while the rendered view is wanted.
#[derive(Default)]
pub(crate) struct MarkdownView {
    cache: CommonMarkCache,
    source: bool,
    // A `Weak` keeps the allocation reserved, so a later document cannot reuse the address.
    built_from: Option<Weak<PreparedDocument>>,
    text: String,
    too_large: bool,
}

impl MarkdownView {
    pub(crate) fn is_source(&self) -> bool {
        self.source
    }

    #[cfg(test)]
    pub(crate) fn has_text(&self) -> bool {
        !self.text.is_empty() || self.built_from.is_some()
    }

    pub(crate) fn is_too_large(&self) -> bool {
        self.too_large
    }

    pub(crate) fn set_source(&mut self, source: bool) {
        self.source = source;
        if source {
            self.clear();
        }
    }

    /// Drops the rendered text without changing the chosen mode.
    pub(crate) fn clear(&mut self) {
        self.text = String::new();
        self.built_from = None;
        self.too_large = false;
    }

    /// Decides what to show and rebuilds the text when the document changed.
    pub(crate) fn resolve(&mut self, document: &Arc<PreparedDocument>) -> MarkdownDisplay {
        if self.source || !document.is_document {
            return MarkdownDisplay::Source;
        }
        let current = self
            .built_from
            .as_ref()
            .is_some_and(|built| std::ptr::eq(built.as_ptr(), Arc::as_ptr(document)));
        if !current {
            let rows = document
                .document
                .comparisons
                .first()
                .map_or(&[][..], |comparison| comparison.rows.as_slice());
            let bytes = rows.iter().map(|row| row.right.len() + 1).sum::<usize>();
            self.too_large = !fits_render_limit(bytes);
            self.text = String::new();
            if !self.too_large {
                self.text.reserve(bytes);
                for (index, row) in rows.iter().enumerate() {
                    if index > 0 {
                        self.text.push('\n');
                    }
                    self.text.push_str(&row.right);
                }
            }
            self.built_from = Some(Arc::downgrade(document));
        }
        if self.too_large {
            MarkdownDisplay::TooLarge
        } else {
            MarkdownDisplay::Rendered
        }
    }

    /// Draws the preview. `id` must stay stable across reloads so the scroll position survives.
    /// `font_size` is the source font size; the preview text scales with it.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, id: impl std::hash::Hash, font_size: f32) {
        let first_new = ui.ctx().output(|output| output.commands.len());
        let family = super::markdown_font::preview_font_family(ui.ctx());
        let (cache, text) = (&mut self.cache, &self.text);
        egui::ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Margin cannot center a column in a wide window, so place the column explicitly.
                let (left, width) = column_layout(ui.available_width());
                let origin = ui.available_rect_before_wrap().min + egui::vec2(left, 0.0);
                let column = egui::Rect::from_min_size(origin, egui::vec2(width, f32::INFINITY));
                ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                    apply_preview_style(ui.style_mut(), font_size, family);
                    ui.add_space(PREVIEW_VERTICAL_MARGIN);
                    CommonMarkViewer::new().show(ui, cache, text);
                    ui.add_space(PREVIEW_VERTICAL_MARGIN);
                });
            });
        ui.ctx()
            .output_mut(|output| drop_unsafe_links(&mut output.commands, first_new));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastdiff_core::CancelToken;

    fn document(text: &str) -> Arc<PreparedDocument> {
        Arc::new(PreparedDocument::from_text(
            "doc.md".into(),
            text.into(),
            &CancelToken::default(),
        ))
    }

    #[test]
    fn markdown_paths_match_case_insensitively_by_final_extension() {
        for path in [
            "a.md",
            "/x/README.MD",
            "notes.markdown",
            "C:\\d\\a.Markdown",
        ] {
            assert!(is_markdown_path(path), "{path}");
        }
        for path in ["a.txt", "README", "readme.md.txt", ".md", "dir.md/file", ""] {
            assert!(!is_markdown_path(path), "{path}");
        }
    }

    #[test]
    fn markdown_links_open_only_http_https_and_mailto() {
        for url in [
            "http://a.io",
            "HTTPS://a.io/x",
            "mailto:a@b.c",
            "MailTo:a@b.c",
        ] {
            assert!(is_openable_url(url), "{url}");
        }
        for url in [
            "file:///etc/passwd",
            "other.md",
            "./other.md#top",
            "javascript:alert(1)",
            "data:text/html,x",
            " http://a.io",
            "http:",
            "",
        ] {
            assert!(!is_openable_url(url), "{url}");
        }
    }

    #[test]
    fn markdown_filter_drops_only_new_unsafe_open_url_commands() {
        let open = |url: &str| egui::OutputCommand::OpenUrl(egui::OpenUrl::same_tab(url));
        let mut commands = vec![
            open("file:///old"),
            egui::OutputCommand::CopyText("kept".into()),
            open("other.md"),
            open("https://a.io"),
            open("javascript:x"),
        ];
        drop_unsafe_links(&mut commands, 2);
        assert_eq!(
            commands,
            vec![
                open("file:///old"),
                egui::OutputCommand::CopyText("kept".into()),
                open("https://a.io"),
            ]
        );
    }

    #[test]
    fn markdown_size_limit_is_inclusive() {
        assert!(fits_render_limit(MARKDOWN_RENDER_LIMIT_BYTES));
        assert!(!fits_render_limit(MARKDOWN_RENDER_LIMIT_BYTES + 1));
    }

    #[test]
    fn markdown_text_is_rebuilt_only_for_a_new_document_and_dropped_in_source() {
        let mut view = MarkdownView::default();
        let first = document("# a\n\nb");
        assert_eq!(view.resolve(&first), MarkdownDisplay::Rendered);
        assert_eq!(view.text, "# a\n\nb");
        view.text.push('!');
        assert_eq!(view.resolve(&first), MarkdownDisplay::Rendered);
        assert!(view.text.ends_with('!'), "same document keeps its text");
        let second = document("changed");
        assert_eq!(view.resolve(&second), MarkdownDisplay::Rendered);
        assert_eq!(view.text, "changed");
        view.set_source(true);
        assert!(view.text.is_empty() && view.built_from.is_none());
        assert_eq!(view.resolve(&second), MarkdownDisplay::Source);
        view.set_source(false);
        assert_eq!(view.resolve(&second), MarkdownDisplay::Rendered);
        assert_eq!(view.text, "changed");
    }

    #[test]
    fn markdown_over_the_limit_falls_back_without_building_text() {
        let mut view = MarkdownView::default();
        let large = document(&"x".repeat(MARKDOWN_RENDER_LIMIT_BYTES + 1));
        assert_eq!(view.resolve(&large), MarkdownDisplay::TooLarge);
        assert!(view.is_too_large() && view.text.is_empty());
        let small = document("fits");
        assert_eq!(view.resolve(&small), MarkdownDisplay::Rendered);
        assert!(!view.is_too_large());
    }

    #[test]
    fn preview_column_fills_narrow_windows_minus_the_margins() {
        let (left, width) = column_layout(500.0);
        assert_eq!(
            (left, width),
            (PREVIEW_SIDE_MARGIN, 500.0 - 2.0 * PREVIEW_SIDE_MARGIN)
        );
    }

    #[test]
    fn preview_column_stops_growing_at_the_cap_and_centers() {
        let at_cap = PREVIEW_MAX_COLUMN_WIDTH + 2.0 * PREVIEW_SIDE_MARGIN;
        assert_eq!(
            column_layout(at_cap),
            (PREVIEW_SIDE_MARGIN, PREVIEW_MAX_COLUMN_WIDTH)
        );
        let (left, width) = column_layout(1400.0);
        assert_eq!(width, PREVIEW_MAX_COLUMN_WIDTH);
        assert!(left > PREVIEW_SIDE_MARGIN);
        assert_eq!(left * 2.0 + width, 1400.0);
    }

    #[test]
    fn preview_column_survives_tiny_and_invalid_widths() {
        for available in [100.0, 10.0, 0.0, -50.0, f32::NAN] {
            let (left, width) = column_layout(available);
            assert!(left >= 0.0 && width >= 0.0, "{available}");
            assert!(left + width <= available.max(0.0), "{available}");
        }
        let (left, width) = column_layout(PREVIEW_MIN_COLUMN_WIDTH);
        assert_eq!((left, width), (0.0, PREVIEW_MIN_COLUMN_WIDTH));
    }

    #[test]
    fn preview_sizes_derive_from_and_scale_with_the_font_size() {
        let (body, mono, heading) = preview_sizes(13.0);
        assert_eq!(body, 13.0 + PREVIEW_BODY_OFFSET);
        assert!(mono < body && body < heading);
        let (zoomed_body, zoomed_mono, zoomed_heading) = preview_sizes(20.0);
        assert!(zoomed_body > body && zoomed_mono > mono && zoomed_heading > heading);
    }

    #[test]
    fn preview_style_changes_only_the_text_sizes_and_block_spacing() {
        let mut style = egui::Style::default();
        let family = style.text_styles[&egui::TextStyle::Body].family.clone();
        let small = style.text_styles[&egui::TextStyle::Small].clone();
        let spacing = style.spacing.item_spacing;
        apply_preview_style(&mut style, 13.0, None);
        let (body, mono, heading) = preview_sizes(13.0);
        assert_eq!(style.text_styles[&egui::TextStyle::Body].size, body);
        assert_eq!(style.text_styles[&egui::TextStyle::Body].family, family);
        assert_eq!(style.text_styles[&egui::TextStyle::Monospace].size, mono);
        assert_eq!(style.text_styles[&egui::TextStyle::Heading].size, heading);
        assert_eq!(style.text_styles[&egui::TextStyle::Small], small);
        assert_eq!(style.spacing.item_spacing.x, spacing.x);
        assert_eq!(
            style.spacing.item_spacing.y,
            spacing.y + PREVIEW_BLOCK_SPACING
        );
    }

    #[test]
    fn preview_style_switches_body_and_heading_but_not_monospace_to_the_family() {
        let mut style = egui::Style::default();
        let mono = style.text_styles[&egui::TextStyle::Monospace]
            .family
            .clone();
        let small = style.text_styles[&egui::TextStyle::Small].clone();
        let family = egui::FontFamily::Name("fastdiff-preview".into());
        apply_preview_style(&mut style, 13.0, Some(family.clone()));
        assert_eq!(style.text_styles[&egui::TextStyle::Body].family, family);
        assert_eq!(style.text_styles[&egui::TextStyle::Heading].family, family);
        assert_eq!(style.text_styles[&egui::TextStyle::Monospace].family, mono);
        assert_eq!(style.text_styles[&egui::TextStyle::Small], small);
    }

    #[test]
    fn preview_draws_headings_while_the_preview_font_is_not_ready() {
        // Marks the font as installed in a pass that has not finished yet, the state right after
        // the first Windows preview frame. On other systems the bundled fonts are used directly.
        let view = MarkdownView {
            text: "# Title\n\nbody `code`".into(),
            ..MarkdownView::default()
        };
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, view: &mut MarkdownView| {
                super::super::markdown_font::mark_install_pending(ui.ctx());
                view.show(ui, "id", 13.0);
            },
            view,
        );
        harness.run();
        use egui_kittest::kittest::Queryable;
        harness.get_by_label("Title");
    }
}

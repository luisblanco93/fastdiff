use super::Side;
use super::{
    interaction::SourceCell,
    selection::{Pane, Point, Selection, source_text},
};
pub(super) use crate::worker::character_columns;
use crate::{
    highlight::{PREVIEW_SUFFIX, Span, TokenKind, preview_len},
    theme::{Active, CodePalette},
    worker::{LineCheckpoint, PreparedDocument},
};
use egui::{FontId, Layout, RichText, TextFormat, text::LayoutJob};
use fastdiff_core::RowKind;

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_row(
    ui: &mut egui::Ui,
    active: Active,
    document: &PreparedDocument,
    index: usize,
    side: Side,
    unified: bool,
    size: egui::Vec2,
    font: f32,
    offsets: [f32; 2],
    search_match: bool,
    selection: &mut Selection,
    cells: &mut Vec<SourceCell>,
) {
    let width = size.x;
    let height = size.y;
    let prepared = &document.rows[index];
    let row = &document.document.comparisons[prepared.comparison].rows[prepared.index];
    if row.kind == RowKind::Header {
        if search_match {
            let rect = egui::Rect::from_min_size(ui.cursor().min, size);
            ui.painter()
                .rect_filled(rect, 0.0, active.code_palette().highlight);
        }
        let text = if row.right.is_empty() {
            &row.left
        } else {
            &row.right
        };
        ui.add_sized(
            [width, height],
            egui::Label::new(
                RichText::new(text)
                    .monospace()
                    .size(font)
                    .color(active.app_theme().header),
            )
            .selectable(false)
            .truncate(),
        );
        return;
    }
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let mut row_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("row", index, side as u8))
            .max_rect(rect)
            .layout(Layout::left_to_right(egui::Align::Center)),
    );
    row_ui.set_clip_rect(ui.clip_rect().intersect(rect));
    {
        let ui = &mut row_ui;
        ui.set_min_size(size);
        ui.spacing_mut().item_spacing.x = 0.0;
        if unified {
            let old = side == Side::Old || row.kind == RowKind::Deletion;
            let text = if old { &row.left } else { &row.right };
            let spans = if old { &prepared.left } else { &prepared.right };
            let marker = if old {
                "−"
            } else if side == Side::New || row.kind == RowKind::Addition {
                "+"
            } else {
                " "
            };
            code_cell(
                ui,
                active.code_palette(),
                text,
                spans,
                if old { row.old_number } else { row.new_number },
                marker,
                width,
                height,
                font,
                if old {
                    RowKind::Deletion
                } else if marker == "+" {
                    RowKind::Addition
                } else {
                    row.kind
                },
                offsets[0],
                (index, side == Side::New),
                document,
                selection,
                cells,
                Pane::Unified,
                side,
                search_match,
            );
        } else {
            code_cell(
                ui,
                active.code_palette(),
                &row.left,
                &prepared.left,
                row.old_number,
                "",
                width / 2.0,
                height,
                font,
                if matches!(row.kind, RowKind::Change | RowKind::Deletion) {
                    RowKind::Deletion
                } else {
                    RowKind::Context
                },
                offsets[0],
                (index, false),
                document,
                selection,
                cells,
                Pane::Old,
                Side::Both,
                search_match,
            );
            code_cell(
                ui,
                active.code_palette(),
                &row.right,
                &prepared.right,
                row.new_number,
                "",
                width / 2.0,
                height,
                font,
                if matches!(row.kind, RowKind::Change | RowKind::Addition) {
                    RowKind::Addition
                } else {
                    RowKind::Context
                },
                offsets[1],
                (index, true),
                document,
                selection,
                cells,
                Pane::New,
                Side::Both,
                search_match,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn code_cell(
    ui: &mut egui::Ui,
    palette: &CodePalette,
    text: &str,
    spans: &[Span],
    number: Option<usize>,
    marker: &str,
    width: f32,
    height: f32,
    font: f32,
    kind: RowKind,
    offset: f32,
    identity: (usize, bool),
    document: &PreparedDocument,
    selection: &mut Selection,
    cells: &mut Vec<SourceCell>,
    pane: Pane,
    side: Side,
    search_match: bool,
) {
    let background = if search_match {
        palette.highlight
    } else {
        match kind {
            RowKind::Addition => palette.addition,
            RowKind::Deletion => palette.deletion,
            _ => palette.surface,
        }
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, background);
    let gutter = egui::Rect::from_min_size(rect.min, egui::vec2(64.0_f32.min(width), height));
    let mut gutter_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt((identity, "gutter"))
            .max_rect(gutter)
            .layout(Layout::left_to_right(egui::Align::Center)),
    );
    gutter_ui.set_clip_rect(ui.clip_rect().intersect(gutter));
    gutter_ui.add_sized(
        gutter.size(),
        egui::Label::new(
            RichText::new(format!(
                "{} {marker}",
                number.map(|n| n.to_string()).unwrap_or_default(),
            ))
            .monospace()
            .size(font)
            .color(palette.gutter),
        )
        .selectable(false)
        .truncate(),
    );
    let source = egui::Rect::from_min_max(egui::pos2(gutter.right(), rect.top()), rect.max);
    let advance = if document.is_document {
        ui.fonts_mut(|fonts| fonts.glyph_width(&FontId::monospace(font), ' '))
    } else {
        font * 0.61
    };
    let (window, character_start, columns) = if document.is_document {
        document_window(
            text,
            offset,
            source.width(),
            advance,
            document.line_checkpoints(identity.0),
        )
    } else {
        (0..preview_len(text), 0, 0)
    };
    let visible_text = &text[window.clone()];
    let text_rect = source.translate(egui::vec2(columns as f32 * advance - offset, 0.0));
    let mut source_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt((identity, "source"))
            .max_rect(text_rect)
            .layout(Layout::left_to_right(egui::Align::Center)),
    );
    source_ui.set_clip_rect(ui.clip_rect().intersect(source));
    let (origin, galley, response) = egui::Label::new(if document.is_document {
        window_layout(text, spans, window.clone(), font, palette)
    } else {
        layout(text, spans, font, palette)
    })
    .extend()
    .selectable(false)
    .layout_in_ui(&mut source_ui);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), galley.text())
    });
    let comparison = document.rows[identity.0].comparison;
    let point = Point {
        row: identity.0,
        side,
        character: 0,
    };
    if source_text(document, comparison, pane, point).is_some() {
        let character_count = visible_text.chars().count();
        if let Some(range) = selection
            .range_for(
                comparison,
                pane,
                point,
                text,
                document
                    .line_character_count(identity.0)
                    .filter(|_| pane != Pane::Old),
            )
            .filter(|range| {
                range.end >= character_start && range.start <= character_start + character_count
            })
        {
            let start = galley
                .pos_from_cursor(egui::text::CCursor::new(
                    range
                        .start
                        .saturating_sub(character_start)
                        .min(character_count),
                ))
                .left();
            let end = galley
                .pos_from_cursor(egui::text::CCursor::new(
                    range
                        .end
                        .saturating_sub(character_start)
                        .min(character_count),
                ))
                .left();
            let highlight = egui::Rect::from_min_max(
                egui::pos2(origin.x + start, source.top()),
                egui::pos2(origin.x + end.max(start + 3.0), source.bottom()),
            );
            source_ui
                .painter()
                .rect_filled(highlight, 0.0, palette.selection);
        }
        let response = ui.interact(
            source,
            ui.id().with((identity, "selection")),
            egui::Sense::click_and_drag() - egui::Sense::FOCUSABLE,
        );
        cells.push(SourceCell {
            rect: source.intersect(ui.clip_rect()),
            origin,
            galley: galley.clone(),
            comparison,
            pane,
            point,
            character_limit: character_count,
            character_start,
            response,
        });
        let cell = cells.last().expect("source cell just inserted");
        cell.interact(ui, document, selection);
    }
    source_ui.painter().galley(origin, galley, palette.text);
}

fn document_window(
    text: &str,
    offset: f32,
    width: f32,
    advance: f32,
    checkpoints: &[LineCheckpoint],
) -> (std::ops::Range<usize>, usize, usize) {
    let first = (offset / advance).max(0.0) as usize;
    let last = ((offset + width.max(0.0)) / advance).ceil() as usize + 8;
    // The endpoint stores the length; resuming before it preserves the final scalar on overscroll.
    let checkpoints = &checkpoints[..checkpoints.len().saturating_sub(1)];
    let index =
        checkpoints.partition_point(|checkpoint| checkpoint.columns <= first.saturating_sub(8));
    let checkpoint = checkpoints
        .get(index.saturating_sub(1))
        .copied()
        .unwrap_or_default();
    let mut columns = checkpoint.columns;
    let mut start = checkpoint.byte;
    let mut character_start = checkpoint.character;
    let mut start_columns = columns;
    let mut end = text.len();
    for (character, (byte, ch)) in text[checkpoint.byte..].char_indices().enumerate() {
        let byte = checkpoint.byte + byte;
        if columns <= first.saturating_sub(8) {
            start = byte;
            character_start = checkpoint.character + character;
            start_columns = columns;
        }
        if columns > last {
            end = byte;
            break;
        }
        columns += character_columns(ch);
    }
    (start..end, character_start, start_columns)
}

fn window_layout(
    text: &str,
    spans: &[Span],
    window: std::ops::Range<usize>,
    font: f32,
    palette: &CodePalette,
) -> LayoutJob {
    let spans: Vec<_> = spans
        .iter()
        .filter_map(|span| {
            let start = span.start.max(window.start);
            let end = span.end.min(window.end);
            (start < end).then_some(Span {
                start: start - window.start,
                end: end - window.start,
                kind: span.kind,
            })
        })
        .collect();
    let visible = &text[window];
    // The window is bounded by viewport columns, so it can be laid out without preview truncation.
    layout_range(visible, &spans, font, visible.len(), false, palette)
}

fn layout(text: &str, spans: &[Span], font: f32, palette: &CodePalette) -> LayoutJob {
    let end = preview_len(text);
    layout_range(text, spans, font, end, end < text.len(), palette)
}

fn layout_range(
    text: &str,
    spans: &[Span],
    font: f32,
    end: usize,
    suffix: bool,
    palette: &CodePalette,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let normal = TextFormat {
        font_id: FontId::monospace(font),
        color: palette.text,
        ..Default::default()
    };
    let mut cursor = 0;
    for span in spans {
        job.append(&text[cursor..span.start], 0.0, normal.clone());
        let color = match span.kind {
            TokenKind::Keyword => palette.syntax.keyword,
            TokenKind::String => palette.syntax.string,
            TokenKind::Comment => palette.syntax.comment,
            TokenKind::Number => palette.syntax.number,
        };
        job.append(
            &text[span.start..span.end],
            0.0,
            TextFormat {
                color,
                ..normal.clone()
            },
        );
        cursor = span.end;
    }
    job.append(&text[cursor..end], 0.0, normal.clone());
    if suffix {
        job.append(PREVIEW_SUFFIX, 0.0, normal);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_spans_use_the_palette_colors_in_every_theme() {
        let text = "if \"a\" 1 // c";
        let spans = [
            Span {
                start: 0,
                end: 2,
                kind: TokenKind::Keyword,
            },
            Span {
                start: 3,
                end: 6,
                kind: TokenKind::String,
            },
            Span {
                start: 7,
                end: 8,
                kind: TokenKind::Number,
            },
            Span {
                start: 9,
                end: text.len(),
                kind: TokenKind::Comment,
            },
        ];
        for id in crate::theme::ThemeId::ALL {
            let palette = &id.theme().code;
            let expected = [
                palette.syntax.keyword,
                palette.syntax.string,
                palette.syntax.number,
                palette.syntax.comment,
            ];
            for job in [
                layout(text, &spans, 13.0, palette),
                window_layout(text, &spans, 0..text.len(), 13.0, palette),
            ] {
                for (span, color) in spans.iter().zip(expected) {
                    let section = job
                        .sections
                        .iter()
                        .find(|section| section.byte_range == (span.start..span.end))
                        .unwrap();
                    assert_eq!(section.format.color, color, "{id:?} {:?}", span.kind);
                }
                assert!(
                    job.sections
                        .iter()
                        .any(|section| section.format.color == palette.text),
                    "{id:?} plain text"
                );
            }
        }
    }

    #[test]
    fn document_windows_reach_long_line_tails_and_keep_unicode_boundaries() {
        let text = format!("{}漢字tail", "é".repeat(20_000));
        let (range, start, columns) = document_window(&text, 20_000.0, 100.0, 1.0, &[]);
        assert!(range.start > crate::highlight::MAX_PREVIEW_BYTES);
        assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        assert!(text[range].contains("tail"));
        assert!(start >= 19_992 && columns >= 19_992);
    }

    #[test]
    fn windowed_selection_copies_the_complete_document() {
        let text = format!("{}tail", "x".repeat(20_000));
        let document = PreparedDocument::from_text(
            "long.txt".into(),
            text.clone(),
            &fastdiff_core::CancelToken::default(),
        );
        let mut selection = Selection::default();
        selection.select_all(&document, 0, Pane::Unified);
        assert_eq!(selection.text(&document), text);
    }

    #[test]
    fn full_file_checkpoints_index_only_long_utf8_lines_and_keep_scalar_boundaries() {
        let short = "x".repeat(4096);
        let long = "é\t漢😀\u{301}".repeat(901);
        let text = format!("{short}\n{long}\nplain\n{}", "é".repeat(2049));
        let document = PreparedDocument::from_text(
            "long.txt".into(),
            text,
            &fastdiff_core::CancelToken::default(),
        );
        assert!(document.line_checkpoints(0).is_empty());
        assert!(document.line_checkpoints(2).is_empty());
        assert!(document.line_checkpoints(4).is_empty());
        for row in [1, 3] {
            let text = &document.document.comparisons[0].rows[row].right;
            let checkpoints = document.line_checkpoints(row);
            assert_eq!(checkpoints[0].byte, 0);
            assert_eq!(checkpoints.last().unwrap().byte, text.len());
            assert_eq!(checkpoints.last().unwrap().character, text.chars().count());
            for checkpoint in checkpoints {
                assert_eq!(checkpoint.row, row);
                assert!(text.is_char_boundary(checkpoint.byte));
                let prefix = &text[..checkpoint.byte];
                assert_eq!(checkpoint.character, prefix.chars().count());
                assert_eq!(
                    checkpoint.columns,
                    prefix.chars().map(character_columns).sum::<usize>()
                );
            }
            assert!(checkpoints.windows(2).all(|pair| {
                pair[1].character - pair[0].character <= 256
                    && pair[1].character > pair[0].character
            }));
        }
        assert_eq!(
            document.right_columns,
            long.chars().map(character_columns).sum::<usize>()
        );
    }

    #[test]
    fn indexed_document_windows_match_linear_windows_at_boundaries_and_overscroll() {
        let text = format!("{}tail", "é\t漢😀\u{301}".repeat(1000));
        let document = PreparedDocument::from_text(
            "long.txt".into(),
            text.clone(),
            &fastdiff_core::CancelToken::default(),
        );
        let checkpoints = document.line_checkpoints(0);
        let total_columns = document.right_columns;
        let mut offsets = vec![-10.0, 0.0, 7.0, 8.0, 100.0, total_columns as f32];
        offsets.push(total_columns as f32 + 1000.0);
        for checkpoint in checkpoints.iter().step_by(3) {
            offsets.extend([
                checkpoint.columns as f32 + 7.0,
                checkpoint.columns as f32 + 8.0,
                checkpoint.columns as f32 + 9.0,
            ]);
        }
        for offset in offsets {
            for width in [-10.0, 0.0, 100.0] {
                assert_eq!(
                    document_window(&text, offset, width, 1.0, checkpoints),
                    document_window(&text, offset, width, 1.0, &[]),
                    "offset={offset}, width={width}"
                );
            }
        }
        let (range, character, _) = document_window(
            &text,
            total_columns as f32 + 1000.0,
            100.0,
            1.0,
            checkpoints,
        );
        assert_eq!(&text[range], "l");
        assert_eq!(character, text.chars().count() - 1);
    }

    #[test]
    fn cancelled_full_file_preparation_leaves_no_partial_checkpoints() {
        let token = fastdiff_core::CancelToken::default();
        token.cancel();
        let text = "漢😀\t".repeat(3000);
        let document = PreparedDocument::from_text("long.txt".into(), text.clone(), &token);
        assert!(document.long_line_checkpoints.is_empty());
        assert_eq!(document.line_character_count(0), None);
        let mut selection = Selection::default();
        selection.select_all(&document, 0, Pane::Unified);
        assert_eq!(selection.text(&document), text);
    }
}

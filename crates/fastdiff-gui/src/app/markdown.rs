//! Preview/Source wiring for Markdown documents; the view itself lives in `render::markdown`.

use super::{ReviewTab, shortcuts};
use crate::render::markdown::is_markdown_path;
use fastdiff_cli::OpenTarget;
use fastdiff_core::GitPath;

impl ReviewTab {
    /// The file shown by the tab: the opened document, or the selected file of a repository.
    pub(super) fn document_location(&self) -> Option<String> {
        if self.request.target == OpenTarget::Document {
            Some(self.request.repository.clone())
        } else {
            self.selected.as_ref().map(GitPath::display)
        }
    }

    /// Only a full-file view of a Markdown file has a rendered preview.
    pub(super) fn markdown_eligible(&self) -> bool {
        self.document_mode
            && self
                .document_location()
                .is_some_and(|path| is_markdown_path(&path))
    }

    /// A Markdown file selected in the changes view, which can be opened rendered.
    pub(super) fn markdown_selectable(&self) -> bool {
        self.request.target == OpenTarget::Repository
            && self.has_repository
            && !self.document_mode
            && self
                .selected
                .as_ref()
                .is_some_and(|path| is_markdown_path(&path.display()))
    }

    /// Raw rows are what search and jumps operate on, so those entry points call this first.
    pub(super) fn show_markdown_source(&mut self) {
        if self.markdown_eligible() {
            self.markdown.set_source(true);
        }
    }

    /// Opens the working-tree file of the selected Markdown file rendered.
    pub(super) fn open_markdown_rendered(&mut self) {
        if self.markdown_selectable() && self.full_available {
            self.markdown.set_source(false);
            self.toggle_document(true);
        }
    }

    /// Plain Full file keeps showing the raw text, also for Markdown.
    pub(super) fn open_full_file(&mut self) {
        if self.markdown_selectable() {
            self.markdown.set_source(true);
        }
        self.toggle_document(true);
    }

    pub(super) fn toggle_markdown(&mut self) {
        if self.markdown_selectable() {
            self.open_markdown_rendered();
        } else if self.markdown_eligible() {
            self.markdown.set_source(!self.markdown.is_source());
        }
    }

    pub(super) fn markdown_mode_buttons(&mut self, ui: &mut egui::Ui) {
        let rendered = !self.markdown.is_source() && !self.markdown.is_too_large();
        // The row is right to left, so Source is added first and appears last.
        for (label, wants_rendered) in [("Source", false), ("Preview", true)] {
            if ui
                .add(egui::Button::selectable(rendered == wants_rendered, label))
                .on_hover_text(Self::markdown_hover())
                .clicked()
            {
                self.markdown.set_source(!wants_rendered);
            }
        }
    }

    /// Opens the selected Markdown file rendered; disabled while Full file is unavailable.
    pub(super) fn markdown_open_button(&mut self, ui: &mut egui::Ui) {
        let response = ui
            .add_enabled(
                self.full_available,
                egui::Button::selectable(false, "Preview"),
            )
            .on_hover_text(Self::markdown_hover());
        if response.clicked() {
            self.open_markdown_rendered();
        }
    }

    fn markdown_hover() -> String {
        shortcuts::binding_label("Markdown preview (Ctrl+Shift+V)")
    }
}

use super::{DisplayLine, PreparedDocument};

const GUTTER: f32 = 64.0;

#[derive(Clone, Copy, Default)]
pub(crate) struct HorizontalOffsets {
    pub split: [f32; 2],
    pub unified: f32,
    pub document_advance: f32,
    pub arrow_targets: [Option<f32>; 2],
}

pub(super) fn smooth_offset(ui: &egui::Ui, current: f32, target: &mut Option<f32>) -> f32 {
    let Some(destination) = *target else {
        return current;
    };
    let duration = ui.style().scroll_animation.duration.min;
    if (destination - current).abs() < 0.5 || duration == 0.0 {
        *target = None;
        return destination;
    }
    ui.ctx().request_repaint();
    let factor =
        egui::emath::exponential_smooth_factor(0.95, duration, ui.input(|input| input.stable_dt));
    egui::lerp(current..=destination, factor)
}

pub(super) fn source_extent(columns: usize, font_size: f32) -> f32 {
    columns as f32 * font_size * 0.61 + 8.0
}

impl HorizontalOffsets {
    fn extent(self, document: &PreparedDocument, columns: usize, font: f32) -> f32 {
        if document.is_document {
            columns as f32 * self.document_advance + 8.0
        } else {
            source_extent(columns, font)
        }
    }

    pub(super) fn active(self, unified: bool) -> [f32; 2] {
        if unified {
            [self.unified; 2]
        } else {
            self.split
        }
    }

    pub(super) fn clamp(
        &mut self,
        document: &PreparedDocument,
        unified: bool,
        width: f32,
        font: f32,
    ) {
        if unified {
            let maximum = (self.extent(document, document.max_columns, font)
                - (width - GUTTER).max(0.0))
            .max(0.0);
            self.unified = self.unified.clamp(0.0, maximum);
            self.arrow_targets[0] = self.arrow_targets[0].map(|target| target.clamp(0.0, maximum));
        } else {
            for ((offset, target), columns) in self
                .split
                .iter_mut()
                .zip(&mut self.arrow_targets)
                .zip([document.left_columns, document.right_columns])
            {
                let maximum =
                    (source_extent(columns, font) - (width / 2.0 - GUTTER).max(0.0)).max(0.0);
                *offset = offset.clamp(0.0, maximum);
                *target = target.map(|target| target.clamp(0.0, maximum));
            }
        }
    }

    pub(super) fn wheel(&mut self, ui: &egui::Ui, unified: bool, width: f32) {
        let rect = ui.clip_rect();
        let Some(pointer) = ui
            .ctx()
            .pointer_hover_pos()
            .filter(|position| rect.contains(*position))
        else {
            return;
        };
        let delta = ui.ctx().input_mut(|input| {
            let delta = input.smooth_scroll_delta.x;
            input.smooth_scroll_delta.x = 0.0;
            delta
        });
        if unified {
            if delta != 0.0 {
                self.arrow_targets[0] = None;
            }
            self.unified = (self.unified - delta).max(0.0);
        } else {
            let pane = usize::from(pointer.x > rect.left() + width / 2.0);
            if delta != 0.0 {
                self.arrow_targets[pane] = None;
            }
            self.split[pane] = (self.split[pane] - delta).max(0.0);
        }
    }

    pub(super) fn show_bars(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        document: &PreparedDocument,
        unified: bool,
        width: f32,
        font: f32,
    ) {
        let count = if unified { 1 } else { 2 };
        let pane_width = width / count as f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::hover());
        for pane in 0..count {
            let start = rect.left() + pane as f32 * pane_width + GUTTER.min(pane_width);
            let source_width = (pane_width - GUTTER).max(0.0);
            let viewport = egui::Rect::from_min_size(
                egui::pos2(start, rect.top()),
                egui::vec2(source_width, rect.height()),
            );
            let mut bar_ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt((id, "bar", unified, pane))
                    .max_rect(viewport),
            );
            let columns = if unified {
                document.max_columns
            } else if pane == 0 {
                document.left_columns
            } else {
                document.right_columns
            };
            let offset = if unified {
                self.unified
            } else {
                self.split[pane]
            };
            let output = egui::ScrollArea::horizontal()
                .id_salt((id, "horizontal", unified, pane))
                .auto_shrink([false, false])
                .max_height(20.0)
                .min_scrolled_height(0.0)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .horizontal_scroll_offset(offset)
                .show(&mut bar_ui, |ui| {
                    ui.allocate_exact_size(
                        egui::vec2(self.extent(document, columns, font).max(source_width), 1.0),
                        egui::Sense::hover(),
                    );
                });
            if output.state.offset.x != offset {
                self.arrow_targets[pane] = None;
                ui.ctx().request_repaint();
            }
            if unified {
                self.unified = output.state.offset.x;
            } else {
                self.split[pane] = output.state.offset.x;
            }
        }
    }
}

pub(super) fn anchor_index(lines: &[DisplayLine], anchor: &DisplayLine) -> usize {
    lines
        .iter()
        .position(|line| match (anchor, line) {
            (
                DisplayLine::Source {
                    row: old,
                    side: old_side,
                },
                DisplayLine::Source { row, side },
            ) => old == row && old_side == side,
            (DisplayLine::Comparison(old), DisplayLine::Comparison(comparison)) => {
                old == comparison
            }
            (DisplayLine::Fold { start: old, .. }, DisplayLine::Fold { start, .. }) => old == start,
            (DisplayLine::Limit { .. }, DisplayLine::Limit { .. }) => true,
            _ => false,
        })
        .or_else(|| match anchor {
            DisplayLine::Source { row: old, .. } => lines
                .iter()
                .position(|line| matches!(line, DisplayLine::Source { row, .. } if row == old)),
            _ => None,
        })
        .unwrap_or_else(|| lines.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastdiff_core::CancelToken;

    #[test]
    fn document_scroll_extent_and_clamp_use_the_measured_glyph_advance() {
        let document = PreparedDocument::from_text(
            "long.txt".into(),
            "x".repeat(20_000),
            &CancelToken::default(),
        );
        let mut offsets = HorizontalOffsets {
            unified: f32::MAX,
            document_advance: 9.0,
            ..Default::default()
        };
        assert_eq!(offsets.extent(&document, 20_000, 13.0), 180_008.0);
        offsets.clamp(&document, true, 800.0, 13.0);
        assert_eq!(offsets.unified, 180_008.0 - (800.0 - GUTTER));
        // Changing font size uses the new measured advance, rather than the old estimate.
        offsets.document_advance = 11.0;
        offsets.unified = f32::MAX;
        offsets.clamp(&document, true, 800.0, 19.0);
        assert_eq!(offsets.unified, 220_008.0 - (800.0 - GUTTER));
    }
}

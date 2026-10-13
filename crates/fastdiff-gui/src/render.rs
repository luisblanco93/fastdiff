use crate::{
    presentation::{VisibleRow, visible_rows},
    worker::PreparedDocument,
};
use egui::{Color32, RichText};
use fastdiff_core::RowKind;

mod cells;
mod interaction;
mod responsive;
pub(crate) mod selection;
#[cfg(test)]
mod selection_tests;
use cells::draw_row;
use responsive::{HorizontalOffsets, anchor_index, smooth_offset};

const UNIFIED_BELOW_WIDTH: f32 = 640.0;
const SPLIT_AT_WIDTH: f32 = 680.0;
use std::{collections::HashSet, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Both,
    Old,
    New,
}

#[derive(Clone, Debug)]
pub enum DisplayLine {
    Comparison(usize),
    Source { row: usize, side: Side },
    Fold { start: usize, end: usize },
    Limit { omitted: usize },
}

pub struct DiffPresentation {
    pub document: Arc<PreparedDocument>,
    pub lines: Arc<[DisplayLine]>,
    pub expanded: HashSet<usize>,
    pub unified: bool,
    pub scroll_y: f32,
    pub jump: Option<usize>,
    pub(crate) horizontal: HorizontalOffsets,
    font_size: f32,
    pending_scroll: Option<f32>,
    arrow_target: Option<f32>,
    pending_match_columns: Option<usize>,
    narrow: bool,
    pub(crate) selection: selection::Selection,
    pub(crate) search_hits: Vec<usize>,
}

impl DiffPresentation {
    pub fn new(document: Arc<PreparedDocument>, unified: bool) -> Self {
        let unified = unified || document.is_document;
        let lines = Arc::clone(if unified {
            &document.unified_lines
        } else {
            &document.side_lines
        });
        Self {
            document,
            lines,
            expanded: HashSet::new(),
            unified,
            scroll_y: 0.0,
            jump: None,
            horizontal: HorizontalOffsets::default(),
            font_size: 13.0,
            pending_scroll: None,
            arrow_target: None,
            pending_match_columns: None,
            narrow: false,
            selection: selection::Selection::default(),
            search_hits: Vec::new(),
        }
    }

    pub fn preserve_state_from(&mut self, previous: &Self) {
        self.expanded = previous.expanded.clone();
        self.unified = previous.unified;
        self.narrow = previous.narrow;
        self.font_size = previous.font_size;
        self.horizontal = previous.horizontal;
        self.horizontal.arrow_targets = [None; 2];
        self.rebuild();
        self.scroll_y = previous.scroll_y;
        self.pending_scroll = Some(self.scroll_y);
    }

    pub fn set_layout(&mut self, manual_unified: bool, width: f32, font_size: f32) {
        self.narrow = if self.narrow {
            width < SPLIT_AT_WIDTH
        } else {
            width < UNIFIED_BELOW_WIDTH
        };
        let unified = self.document.is_document || manual_unified || self.narrow;
        let previous_height = self.font_size + 5.0;
        let current = (self.scroll_y / previous_height) as usize;
        let fraction = (self.scroll_y % previous_height) / previous_height;
        let anchor = self.lines.get(current).cloned();
        let jump_anchor = self.jump.and_then(|index| self.lines.get(index).cloned());
        if unified != self.unified || font_size != self.font_size {
            self.arrow_target = None;
            self.horizontal.arrow_targets = [None; 2];
            if unified != self.unified {
                self.clear_selection();
            }
            self.unified = unified;
            self.font_size = font_size;
            self.rebuild();
            let index = anchor
                .as_ref()
                .map(|anchor| anchor_index(&self.lines, anchor))
                .unwrap_or(0);
            self.scroll_y = (index as f32 + fraction) * (font_size + 5.0);
            self.pending_scroll = Some(self.scroll_y);
            self.jump = jump_anchor
                .as_ref()
                .map(|anchor| anchor_index(&self.lines, anchor));
        }
        if !self.document.is_document {
            self.horizontal
                .clamp(&self.document, unified, width, font_size);
        }
    }

    pub fn rebuild(&mut self) {
        self.lines = if self.expanded.is_empty() {
            Arc::clone(if self.unified {
                &self.document.unified_lines
            } else {
                &self.document.side_lines
            })
        } else {
            make_lines(&self.document, self.unified, &self.expanded).into()
        };
    }

    pub fn clear_selection(&mut self) {
        self.selection = selection::Selection::default();
    }

    pub fn navigate_hunk(&mut self, next: bool, row_height: f32) {
        let current = (self.scroll_y / row_height) as usize;
        let hunks: Vec<usize> = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(line, item)| match item {
                DisplayLine::Source { row, .. } => {
                    let prepared = &self.document.rows[*row];
                    (self.document.document.comparisons[prepared.comparison].rows[prepared.index]
                        .kind
                        == RowKind::Header)
                        .then_some(line)
                }
                _ => None,
            })
            .collect();
        self.jump = if next {
            hunks
                .iter()
                .copied()
                .find(|line| *line > current + 1)
                .or_else(|| hunks.first().copied())
        } else {
            hunks
                .iter()
                .rev()
                .copied()
                .find(|line| *line < current)
                .or_else(|| hunks.last().copied())
        };
    }

    pub fn jump_to_source(&mut self, source: usize) {
        if let Some(DisplayLine::Fold { start, .. }) = self.lines.iter().find(|line| matches!(line, DisplayLine::Fold { start, end } if *start <= source && source < *end)) {
            self.expanded.insert(*start); self.rebuild();
        }
        self.jump = self
            .lines
            .iter()
            .position(|line| matches!(line, DisplayLine::Source { row, .. } if *row == source));
    }

    pub fn jump_to_match(&mut self, source: usize, query: &str) {
        self.jump_to_source(source);
        if !self.document.is_document || query.is_empty() {
            return;
        }
        let Some(prepared) = self.document.rows.get(source) else {
            return;
        };
        let text =
            &self.document.document.comparisons[prepared.comparison].rows[prepared.index].right;
        let needle = query.to_lowercase();
        if let Some(byte) = text.to_lowercase().find(&needle) {
            // Lowercasing can expand characters, so map back through the original text.
            let mut lowered = 0;
            let columns: usize = text
                .chars()
                .take_while(|ch| {
                    let before = lowered;
                    lowered += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
                    before < byte
                })
                .map(cells::character_columns)
                .sum();
            self.pending_match_columns = Some(columns);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, id: egui::Id, font_size: f32) {
        self.show_tracked(ui, id, font_size, None);
    }

    pub(crate) fn show_tracked(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        font_size: f32,
        marker: Option<&str>,
    ) -> bool {
        let row_height = font_size + 5.0;
        if self.document.is_document {
            self.horizontal.document_advance =
                ui.fonts_mut(|fonts| fonts.glyph_width(&egui::FontId::monospace(font_size), ' '));
        }
        if let Some(columns) = self.pending_match_columns.take() {
            self.horizontal.arrow_targets = [None; 2];
            self.horizontal.unified =
                (columns as f32 * self.horizontal.document_advance - 24.0).max(0.0);
        }
        let height = (ui.available_height() - 20.0).max(0.0);
        if self.pending_scroll.is_some()
            || self.jump.is_some()
            || ui.input(|input| input.pointer.primary_down() || input.smooth_scroll_delta.y != 0.0)
        {
            self.arrow_target = None;
        }
        if let Some(target) = &mut self.arrow_target {
            *target = target.clamp(
                0.0,
                (self.lines.len() as f32 * row_height - height).max(0.0),
            );
            self.pending_scroll = Some(smooth_offset(ui, self.scroll_y, &mut self.arrow_target));
        }
        if ui.input(|input| input.pointer.primary_down()) {
            self.horizontal.arrow_targets = [None; 2];
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(height)
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: false,
                ..Default::default()
            })
            .auto_shrink([false, false]);
        if let Some(offset) = self.pending_scroll.take() {
            area = area.vertical_scroll_offset(offset);
        }
        if let Some(jump) = self.jump.take() {
            area = area.vertical_scroll_offset(jump as f32 * row_height);
        }
        let mut expand = None;
        let mut cells = Vec::new();
        let output = area.show_rows(ui, row_height, self.lines.len(), |ui, visible| {
            let width = ui.available_width();
            self.horizontal
                .clamp(&self.document, self.unified, width, font_size);
            self.horizontal.wheel(ui, self.unified, width);
            self.horizontal
                .clamp(&self.document, self.unified, width, font_size);
            if self.unified {
                self.horizontal.unified = smooth_offset(
                    ui,
                    self.horizontal.unified,
                    &mut self.horizontal.arrow_targets[0],
                );
            } else {
                for (offset, target) in self
                    .horizontal
                    .split
                    .iter_mut()
                    .zip(&mut self.horizontal.arrow_targets)
                {
                    *offset = smooth_offset(ui, *offset, target);
                }
            }
            let offsets = self.horizontal.active(self.unified);
            for index in visible {
                match &self.lines[index] {
                    DisplayLine::Comparison(comparison) => {
                        ui.add_sized(
                            [width, row_height],
                            egui::Label::new(
                                RichText::new(
                                    &self.document.document.comparisons[*comparison].label,
                                )
                                .strong()
                                .color(Color32::from_rgb(147, 180, 239)),
                            )
                            .selectable(false)
                            .truncate(),
                        );
                    }
                    DisplayLine::Fold { start, end } => {
                        if ui
                            .add_sized(
                                [width, row_height],
                                egui::Button::new(format!("Show {} context lines", end - start))
                                    .frame(false),
                            )
                            .clicked()
                        {
                            expand = Some(*start);
                        }
                    }
                    DisplayLine::Source { row, side } => draw_row(
                        ui,
                        &self.document,
                        *row,
                        *side,
                        self.unified,
                        egui::vec2(width, row_height),
                        font_size,
                        offsets,
                        self.search_hits.binary_search(row).is_ok(),
                        &mut self.selection,
                        &mut cells,
                    ),
                    DisplayLine::Limit { omitted } => {
                        ui.add_sized(
                            [width, row_height],
                            egui::Label::new(
                                RichText::new(format!(
                                    "Rendering limited to 100,000 rows; {omitted} rows omitted"
                                ))
                                .color(Color32::from_rgb(229, 177, 113)),
                            )
                            .selectable(false)
                            .truncate(),
                        );
                    }
                }
            }
        });
        self.scroll_y = output.state.offset.y;
        if let Some(delta) = interaction::drag(
            ui,
            &self.document,
            &mut self.selection,
            &cells,
            output.inner_rect,
        ) {
            let maximum =
                (self.lines.len() as f32 * row_height - output.inner_rect.height()).max(0.0);
            self.pending_scroll = Some((self.scroll_y + delta).clamp(0.0, maximum));
        }
        interaction::shortcuts(ui, &self.document, &mut self.selection);
        self.scroll_arrows(ui, output.inner_rect, font_size);
        self.horizontal.show_bars(
            ui,
            id,
            &self.document,
            self.unified,
            output.inner_rect.width(),
            font_size,
        );
        if let Some(start) = expand {
            self.expanded.insert(start);
            self.rebuild();
        }
        ui.is_visible()
            && !ui.is_sizing_pass()
            && marker.is_some_and(|marker| {
                cells.iter().any(|cell| {
                    cell.galley.text().match_indices(marker).any(|(byte, _)| {
                        let start = cell.galley.text()[..byte].chars().count();
                        let end = start + marker.chars().count();
                        let start = cell.galley.pos_from_cursor(egui::text::CCursor::new(start));
                        let end = cell.galley.pos_from_cursor(egui::text::CCursor::new(end));
                        let rect = egui::Rect::from_min_max(
                            cell.origin + start.min.to_vec2(),
                            cell.origin + end.max.to_vec2(),
                        );
                        rect.is_positive() && cell.rect.contains_rect(rect)
                    })
                })
            })
    }
}

pub(crate) fn make_lines(
    document: &PreparedDocument,
    unified: bool,
    expanded: &HashSet<usize>,
) -> Vec<DisplayLine> {
    if document.is_document {
        return (0..document.rows.len())
            .map(|row| DisplayLine::Source {
                row,
                side: Side::Both,
            })
            .collect();
    }
    let mut lines = Vec::with_capacity(document.rows.len() + document.document.comparisons.len());
    let mut comparison = None;
    for visible in visible_rows(&document.context, expanded) {
        match visible {
            VisibleRow::Fold { start, end } => lines.push(DisplayLine::Fold { start, end }),
            VisibleRow::Source(row) => {
                let prepared = &document.rows[row];
                if comparison != Some(prepared.comparison) {
                    comparison = Some(prepared.comparison);
                    lines.push(DisplayLine::Comparison(prepared.comparison));
                }
                let kind =
                    document.document.comparisons[prepared.comparison].rows[prepared.index].kind;
                if unified && kind == RowKind::Change {
                    lines.extend([
                        DisplayLine::Source {
                            row,
                            side: Side::Old,
                        },
                        DisplayLine::Source {
                            row,
                            side: Side::New,
                        },
                    ]);
                } else {
                    lines.push(DisplayLine::Source {
                        row,
                        side: Side::Both,
                    });
                }
            }
        }
    }
    const MAX_RENDER_ROWS: usize = 100_000;
    if lines.len() > MAX_RENDER_ROWS {
        let omitted = lines.len() - (MAX_RENDER_ROWS - 1);
        lines.truncate(MAX_RENDER_ROWS - 1);
        lines.push(DisplayLine::Limit { omitted });
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::responsive::source_extent;
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    use fastdiff_core::{CancelToken, Comparison, DiffDocument, DiffRow};

    pub(super) fn prepare(rows: Vec<DiffRow>) -> Arc<PreparedDocument> {
        Arc::new(PreparedDocument::new(
            DiffDocument {
                comparisons: vec![Comparison {
                    label: "Unstaged".into(),
                    patch: String::new(),
                    summary: None,
                    rows,
                }],
            },
            &CancelToken::default(),
        ))
    }
    pub(super) fn row(kind: RowKind, number: usize) -> DiffRow {
        DiffRow {
            kind,
            old_number: Some(number),
            new_number: Some(number),
            left: format!("let old_{number} = 1;"),
            right: format!("let new_{number} = 2;"),
        }
    }

    #[test]
    fn tracked_marker_requires_a_visible_source_cell() {
        let document = Arc::new(PreparedDocument::from_text(
            "plain.txt".into(),
            format!(
                "first marker {}horizontal marker\n{}\nlast marker",
                "x".repeat(200),
                "filler\n".repeat(100)
            ),
            &CancelToken::default(),
        ));
        let mut presentation = DiffPresentation::new(document, true);
        let context = egui::Context::default();
        let mut visible = [false; 4];
        for (index, marker) in [
            "first marker",
            "last marker",
            "missing marker",
            "horizontal marker",
        ]
        .iter()
        .enumerate()
        {
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 300.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        visible[index] =
                            presentation.show_tracked(ui, egui::Id::new(index), 13.0, Some(marker));
                    });
                },
            );
        }
        assert!(visible[0]);
        assert!(!visible[1]);
        assert!(!visible[2]);
        assert!(!visible[3]);
    }

    #[test]
    fn full_documents_keep_every_line_in_one_pane_without_folding() {
        let document = Arc::new(PreparedDocument::from_text(
            "plain.txt".into(),
            (0..100_000).map(|_| "line").collect::<Vec<_>>().join("\n"),
            &CancelToken::default(),
        ));
        let mut presentation = DiffPresentation::new(document, false);
        presentation.set_layout(false, 1200.0, 13.0);
        assert!(presentation.unified);
        assert_eq!(presentation.lines.len(), 100_000);
        assert!(presentation.lines.iter().all(|line| matches!(
            line,
            DisplayLine::Source {
                side: Side::Both,
                ..
            }
        )));
    }

    #[test]
    fn document_search_reveals_matches_beyond_the_old_line_preview() {
        let document = Arc::new(PreparedDocument::from_text(
            "plain.txt".into(),
            format!("{}İTARGET", "\t".repeat(5_000)),
            &CancelToken::default(),
        ));
        let mut presentation = DiffPresentation::new(document, false);
        presentation.jump_to_match(0, "target");
        assert_eq!(presentation.jump, Some(0));
        assert_eq!(presentation.pending_match_columns, Some(20_001));
    }

    #[test]
    fn side_by_side_keeps_changes_aligned_and_unified_has_both_lines() {
        let document = prepare(vec![row(RowKind::Change, 1)]);
        assert_eq!(document.side_lines.len(), 2);
        assert_eq!(document.unified_lines.len(), 3);
        assert!(matches!(
            document.unified_lines[1],
            DisplayLine::Source {
                side: Side::Old,
                ..
            }
        ));
        assert!(matches!(
            document.unified_lines[2],
            DisplayLine::Source {
                side: Side::New,
                ..
            }
        ));
    }

    #[test]
    fn right_source_column_has_the_same_origin_for_different_left_line_lengths() {
        let document = prepare(vec![
            DiffRow {
                kind: RowKind::Context,
                old_number: Some(1),
                new_number: Some(1),
                left: "short".into(),
                right: "right_first".into(),
            },
            DiffRow {
                kind: RowKind::Context,
                old_number: Some(2),
                new_number: Some(2),
                left: "a considerably longer left source line".into(),
                right: "right_second".into(),
            },
        ]);
        let harness = Harness::new_ui_state(
            |ui, presentation| presentation.show(ui, egui::Id::new("column-alignment"), 13.0),
            DiffPresentation::new(document, false),
        );
        let first = harness.get_by_label("right_first").rect().left();
        let second = harness.get_by_label("right_second").rect().left();
        assert!(
            (first - second).abs() < 0.1,
            "right column shifted from {first} to {second}"
        );
    }

    #[test]
    fn context_fold_expands_when_clicked_and_search_reveals_hidden_source() {
        let document = prepare((1..=20).map(|n| row(RowKind::Context, n)).collect());
        let mut harness = Harness::new_ui_state(
            |ui, presentation| presentation.show(ui, egui::Id::new("fold-test"), 13.0),
            DiffPresentation::new(document, false),
        );
        harness.get_by_label("Show 14 context lines").click();
        harness.run();
        assert_eq!(harness.state().lines.len(), 21);
        harness.state_mut().expanded.clear();
        harness.state_mut().rebuild();
        harness.state_mut().jump_to_source(10);
        assert_eq!(harness.state().lines.len(), 21);
        assert_eq!(harness.state().jump, Some(11));
    }

    #[test]
    fn rendering_large_documents_only_builds_visible_row_shapes() {
        let document = prepare((1..=10000).map(|n| row(RowKind::Change, n)).collect());
        let harness = Harness::new_ui_state(
            |ui, presentation| presentation.show(ui, egui::Id::new("virtual-test"), 13.0),
            DiffPresentation::new(document, false),
        );
        assert!(
            harness.output().shapes.len() < 500,
            "{} shapes indicate off-screen rows were rendered",
            harness.output().shapes.len()
        );
    }

    #[test]
    fn next_and_previous_hunk_wrap_at_document_boundaries() {
        let document = prepare(vec![
            row(RowKind::Header, 1),
            row(RowKind::Change, 2),
            row(RowKind::Header, 10),
            row(RowKind::Change, 11),
        ]);
        let mut presentation = DiffPresentation::new(document, false);
        presentation.navigate_hunk(true, 18.0);
        assert_eq!(presentation.jump, Some(3));
        presentation.scroll_y = 54.0;
        presentation.navigate_hunk(true, 18.0);
        assert_eq!(presentation.jump, Some(1));
        presentation.scroll_y = 0.0;
        presentation.navigate_hunk(false, 18.0);
        assert_eq!(presentation.jump, Some(3));
    }

    #[test]
    fn responsive_mode_has_hysteresis_and_manual_unified_precedence() {
        let mut presentation = DiffPresentation::new(prepare(vec![row(RowKind::Change, 1)]), false);
        presentation.set_layout(false, 639.0, 13.0);
        assert!(presentation.unified);
        presentation.set_layout(false, 650.0, 13.0);
        assert!(presentation.unified);
        presentation.set_layout(false, 680.0, 13.0);
        assert!(!presentation.unified);
        presentation.set_layout(false, 640.0, 13.0);
        assert!(!presentation.unified);
        presentation.set_layout(true, 1000.0, 13.0);
        assert!(presentation.unified);
    }

    #[test]
    fn layout_changes_preserve_source_anchor_and_fractional_row_position() {
        let mut presentation = DiffPresentation::new(
            prepare((1..=40).map(|n| row(RowKind::Change, n)).collect()),
            false,
        );
        presentation.scroll_y = 18.0 * 11.25;
        presentation.jump = Some(15);
        presentation.set_layout(false, 600.0, 13.0);
        let index = (presentation.scroll_y / 18.0) as usize;
        assert!(matches!(
            presentation.lines[index],
            DisplayLine::Source {
                row: 10,
                side: Side::Old
            }
        ));
        assert!((presentation.scroll_y % 18.0 - 4.5).abs() < 0.01);
        assert!(matches!(
            presentation.lines[presentation.jump.unwrap()],
            DisplayLine::Source { row: 14, .. }
        ));
        presentation.set_layout(false, 900.0, 19.0);
        assert!((presentation.scroll_y - 24.0 * 11.25).abs() < 0.01);
    }

    #[test]
    fn horizontal_offsets_are_independent_and_retained_across_modes() {
        let mut source = row(RowKind::Change, 1);
        source.left = "left_".repeat(120);
        source.right = "right_".repeat(120);
        let mut presentation = DiffPresentation::new(prepare(vec![source]), false);
        presentation.horizontal.split = [91.0, 37.0];
        presentation.horizontal.unified = 58.0;
        presentation.set_layout(false, 600.0, 13.0);
        assert_eq!(presentation.horizontal.active(true), [58.0; 2]);
        presentation.set_layout(false, 900.0, 13.0);
        assert_eq!(presentation.horizontal.active(false), [91.0, 37.0]);
        presentation.set_layout(false, 20000.0, 13.0);
        assert_eq!(presentation.horizontal.split, [0.0; 2]);
    }

    #[test]
    fn scrolling_left_source_does_not_move_right_source_or_gutters() {
        let mut source = row(RowKind::Change, 1);
        source.left = "left_".repeat(120);
        source.right = "right_unique".into();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(800.0, 300.0))
            .build_ui_state(
                |ui, presentation| presentation.show(ui, egui::Id::new("independent-scroll"), 13.0),
                DiffPresentation::new(prepare(vec![source.clone()]), false),
            );
        let right_before = harness.get_by_label("right_unique").rect();
        let left_before = harness.get_by_label(&source.left).rect();
        harness.state_mut().horizontal.split[0] = 100.0;
        harness.run();
        assert_eq!(harness.get_by_label("right_unique").rect(), right_before);
        assert!(
            (harness.get_by_label(&source.left).rect().left() - left_before.left() + 100.0).abs()
                < 0.01
        );
    }

    #[test]
    fn horizontal_wheel_moves_only_the_hovered_source_pane() {
        let mut source = row(RowKind::Change, 1);
        source.left = "left_".repeat(120);
        source.right = "right_wheel_unique".into();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(800.0, 300.0))
            .build_ui_state(
                |ui, presentation| presentation.show(ui, egui::Id::new("wheel-scroll"), 13.0),
                DiffPresentation::new(prepare(vec![source.clone()]), false),
            );
        let left = harness.get_by_label(&source.left).rect();
        let right = harness.get_by_label("right_wheel_unique").rect();
        harness.hover_at(egui::pos2(180.0, left.center().y));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-60.0, 0.0),
            modifiers: egui::Modifiers::default(),
        });
        harness.run();
        assert!(harness.state().horizontal.split[0] > 20.0);
        assert!(harness.get_by_label(&source.left).rect().left() < left.left() - 20.0);
        assert_eq!(harness.get_by_label("right_wheel_unique").rect(), right);
        assert_eq!(harness.state().horizontal.split[1], 0.0);
    }

    #[test]
    fn refresh_preserves_expansion_layout_and_horizontal_state() {
        let document = prepare((1..=40).map(|n| row(RowKind::Change, n)).collect());
        let mut previous = DiffPresentation::new(Arc::clone(&document), false);
        previous.set_layout(false, 600.0, 13.0);
        previous.horizontal.split = [12.0, 21.0];
        previous.horizontal.unified = 8.0;
        previous.expanded.insert(2);
        previous.scroll_y = 117.5;
        let mut refreshed = DiffPresentation::new(document, false);
        refreshed.preserve_state_from(&previous);
        assert!(refreshed.unified);
        assert_eq!(refreshed.scroll_y, 117.5);
        assert_eq!(refreshed.expanded, previous.expanded);
        assert_eq!(refreshed.horizontal.split, [12.0, 21.0]);
        assert_eq!(refreshed.horizontal.unified, 8.0);
    }

    #[test]
    fn long_left_lines_keep_the_right_pane_in_the_viewport() {
        let mut source = row(RowKind::Change, 1);
        source.left = "left_".repeat(120);
        source.right = "visible_right".into();
        let harness = Harness::builder()
            .with_size(egui::vec2(800.0, 300.0))
            .build_ui_state(
                |ui, presentation| presentation.show(ui, egui::Id::new("responsive-width"), 13.0),
                DiffPresentation::new(prepare(vec![source]), false),
            );
        let right = harness.get_by_label("visible_right").rect();
        assert!(
            right.left() < 600.0 && right.right() <= 800.0,
            "right source is off-screen: {right:?}"
        );
    }

    #[test]
    fn long_source_lines_have_full_horizontal_scroll_extent() {
        assert!(source_extent(600, 13.0) > 600.0 * 13.0 * 0.61);
        assert!(source_extent(600, 19.0) > source_extent(600, 13.0));
    }

    #[test]
    fn unified_change_expansion_obeys_the_render_row_limit() {
        let document = prepare((1..=50001).map(|n| row(RowKind::Change, n)).collect());
        assert_eq!(document.unified_lines.len(), 100000);
        assert!(matches!(
            document.unified_lines.last(),
            Some(DisplayLine::Limit { omitted: 4 })
        ));
        assert_eq!(document.side_lines.len(), 50002);
    }
}

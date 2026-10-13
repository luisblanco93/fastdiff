use std::ops::Range;

use crate::worker::PreparedDocument;
use fastdiff_core::RowKind;

use super::Side;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Old,
    New,
    Unified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub row: usize,
    pub side: Side,
    pub character: usize,
}

impl Point {
    fn key(self) -> (usize, usize, usize) {
        let side = match self.side {
            Side::Old => 0,
            Side::New => 1,
            Side::Both => 2,
        };
        (self.row, side, self.character)
    }

    fn in_pane(self, pane: Pane) -> Self {
        Self {
            side: if pane == Pane::Unified {
                self.side
            } else {
                Side::Both
            },
            ..self
        }
    }
}

#[derive(Default)]
pub struct Selection {
    pub active: Option<(usize, Pane)>,
    pub range: Option<(Point, Point)>,
    pub dragging: bool,
}

impl Selection {
    pub fn begin(
        &mut self,
        document: &PreparedDocument,
        comparison: usize,
        pane: Pane,
        point: Point,
        shift: bool,
        double_click: bool,
    ) {
        let point = point.in_pane(pane);
        let Some(text) = source_text(document, comparison, pane, point) else {
            return;
        };
        let length = document
            .line_character_count(point.row)
            .filter(|_| pane != Pane::Old);
        let point = Point {
            character: length.map_or_else(
                || text.chars().take(point.character).count(),
                |length| point.character.min(length),
            ),
            ..point
        };
        let anchor = if shift && self.active == Some((comparison, pane)) {
            self.range.map(|range| range.0).unwrap_or(point)
        } else {
            point
        };
        let range = if double_click {
            (
                Point {
                    character: 0,
                    ..point
                },
                Point {
                    character: length.unwrap_or_else(|| text.chars().count()),
                    ..point
                },
            )
        } else {
            (anchor, point)
        };
        *self = Self {
            active: Some((comparison, pane)),
            range: Some(range),
            dragging: !double_click,
        };
    }

    pub fn extend(&mut self, document: &PreparedDocument, point: Point) {
        let Some((comparison, pane)) = self.active else {
            return;
        };
        let Some((anchor, _)) = self.range else {
            return;
        };
        let point = point.in_pane(pane);
        let Some(text) = source_text(document, comparison, pane, point) else {
            return;
        };
        self.range = Some((
            anchor,
            Point {
                character: document
                    .line_character_count(point.row)
                    .filter(|_| pane != Pane::Old)
                    .map_or_else(
                        || text.chars().take(point.character).count(),
                        |length| point.character.min(length),
                    ),
                ..point
            },
        ));
    }

    pub fn select_all(&mut self, document: &PreparedDocument, comparison: usize, pane: Pane) {
        let mut lines = source_lines(document, comparison, pane);
        let range = lines.next().map(|(first, text)| {
            let (last, text) = lines.last().unwrap_or((first, text));
            (
                first,
                Point {
                    character: document
                        .line_character_count(last.row)
                        .filter(|_| pane != Pane::Old)
                        .unwrap_or_else(|| text.chars().count()),
                    ..last
                },
            )
        });
        *self = Self {
            active: Some((comparison, pane)),
            range,
            dragging: false,
        };
    }

    pub fn text(&self, document: &PreparedDocument) -> String {
        let Some((comparison, pane)) = self.active else {
            return String::new();
        };
        source_lines(document, comparison, pane)
            .filter_map(|(point, text)| {
                let range = self.range_for(
                    comparison,
                    pane,
                    point,
                    text,
                    document
                        .line_character_count(point.row)
                        .filter(|_| pane != Pane::Old),
                )?;
                Some(
                    text.chars()
                        .skip(range.start)
                        .take(range.end - range.start)
                        .collect::<String>(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn range_for(
        &self,
        comparison: usize,
        pane: Pane,
        point: Point,
        text: &str,
        length: Option<usize>,
    ) -> Option<Range<usize>> {
        if self.active != Some((comparison, pane)) {
            return None;
        }
        let (anchor, focus) = self.range?;
        if anchor == focus {
            return None;
        }
        let (start, end) = if anchor.key() <= focus.key() {
            (anchor, focus)
        } else {
            (focus, anchor)
        };
        let point = point.in_pane(pane);
        let line = Point {
            character: 0,
            ..point
        }
        .key();
        if line
            < (Point {
                character: 0,
                ..start
            })
            .key()
            || line
                > (Point {
                    character: 0,
                    ..end
                })
                .key()
        {
            return None;
        }
        let length = length.unwrap_or_else(|| text.chars().count());
        Some(
            if point.row == start.row && point.side == start.side {
                start.character.min(length)
            } else {
                0
            }..if point.row == end.row && point.side == end.side {
                end.character.min(length)
            } else {
                length
            },
        )
    }
}

pub fn source_text(
    document: &PreparedDocument,
    comparison: usize,
    pane: Pane,
    point: Point,
) -> Option<&str> {
    let prepared = document.rows.get(point.row)?;
    if prepared.comparison != comparison {
        return None;
    }
    let row = document
        .document
        .comparisons
        .get(comparison)?
        .rows
        .get(prepared.index)?;
    match pane {
        Pane::Old
            if matches!(
                row.kind,
                RowKind::Context | RowKind::Change | RowKind::Deletion
            ) =>
        {
            Some(&row.left)
        }
        Pane::New
            if matches!(
                row.kind,
                RowKind::Context | RowKind::Change | RowKind::Addition
            ) =>
        {
            Some(&row.right)
        }
        Pane::Unified => match (row.kind, point.side) {
            (RowKind::Change, Side::Old) | (RowKind::Deletion, Side::Both) => Some(&row.left),
            (RowKind::Change, Side::New) | (RowKind::Context | RowKind::Addition, Side::Both) => {
                Some(&row.right)
            }
            _ => None,
        },
        _ => None,
    }
}

fn source_lines(
    document: &PreparedDocument,
    comparison: usize,
    pane: Pane,
) -> impl Iterator<Item = (Point, &str)> {
    document
        .rows
        .iter()
        .enumerate()
        .filter(move |(_, row)| row.comparison == comparison)
        .flat_map(move |(row, prepared)| {
            let kind = document.document.comparisons[comparison].rows[prepared.index].kind;
            let sides = if pane == Pane::Unified && kind == RowKind::Change {
                [Some(Side::Old), Some(Side::New)]
            } else {
                [Some(Side::Both), None]
            };
            sides.into_iter().flatten().filter_map(move |side| {
                let point = Point {
                    row,
                    side,
                    character: 0,
                };
                source_text(document, comparison, pane, point).map(|text| (point, text))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{render::Side, worker::PreparedDocument};
    use fastdiff_core::{CancelToken, Comparison, DiffDocument, DiffRow, RowKind};

    fn row(kind: RowKind, left: &str, right: &str) -> DiffRow {
        DiffRow {
            kind,
            old_number: (!matches!(kind, RowKind::Addition | RowKind::Header)).then_some(1),
            new_number: (!matches!(kind, RowKind::Deletion | RowKind::Header)).then_some(1),
            left: left.into(),
            right: right.into(),
        }
    }

    fn document(rows: Vec<DiffRow>) -> PreparedDocument {
        PreparedDocument::new(
            DiffDocument {
                comparisons: vec![Comparison {
                    label: "Unstaged".into(),
                    patch: String::new(),
                    summary: None,
                    rows,
                }],
            },
            &CancelToken::default(),
        )
    }

    fn point(row: usize, character: usize) -> Point {
        Point {
            row,
            side: Side::Both,
            character,
        }
    }

    #[test]
    fn split_copy_is_independent_and_reverse_unicode_offsets_are_characters() {
        let document = document(vec![
            row(RowKind::Header, "@@ header @@", "@@ header @@"),
            row(RowKind::Change, "α😀old", "新😀new"),
            row(RowKind::Addition, "", "added"),
            row(RowKind::Change, "tail old", "tail new"),
        ]);
        let mut selection = Selection::default();
        selection.begin(&document, 0, Pane::Old, point(3, 4), false, false);
        selection.extend(&document, point(1, 1));
        assert_eq!(selection.text(&document), "😀old\ntail");
        assert_eq!(
            selection.range_for(0, Pane::Old, point(1, 0), "α😀old", None),
            Some(1..5)
        );
        assert_eq!(
            selection.range_for(0, Pane::New, point(1, 0), "新😀new", None),
            None
        );
        selection.begin(&document, 0, Pane::New, point(1, 1), false, false);
        selection.extend(&document, point(3, 4));
        assert_eq!(selection.text(&document), "😀new\nadded\ntail");
    }

    #[test]
    fn select_all_keeps_empty_source_lines_but_excludes_headers_and_placeholders() {
        let document = document(vec![
            row(RowKind::Header, "@@ header @@", "@@ header @@"),
            row(RowKind::Deletion, "before", ""),
            row(RowKind::Deletion, "", ""),
            row(RowKind::Addition, "", "added"),
            row(RowKind::Context, "after", "after"),
        ]);
        let mut selection = Selection::default();
        selection.select_all(&document, 0, Pane::Old);
        assert_eq!(selection.text(&document), "before\n\nafter");
        selection.select_all(&document, 0, Pane::New);
        assert_eq!(selection.text(&document), "added\nafter");
        selection.begin(&document, 0, Pane::Old, point(3, 0), false, false);
        assert_eq!(selection.text(&document), "added\nafter");
    }

    #[test]
    fn unified_copy_uses_display_order_and_both_lines_of_changes() {
        let document = document(vec![
            row(RowKind::Context, "context", "context"),
            row(RowKind::Change, "old", "new"),
            row(RowKind::Deletion, "deleted", ""),
            row(RowKind::Addition, "", "added"),
        ]);
        let mut selection = Selection::default();
        selection.select_all(&document, 0, Pane::Unified);
        assert_eq!(
            selection.text(&document),
            "context\nold\nnew\ndeleted\nadded"
        );
        selection.begin(
            &document,
            0,
            Pane::Unified,
            Point {
                row: 1,
                side: Side::New,
                character: 2,
            },
            false,
            false,
        );
        selection.extend(
            &document,
            Point {
                row: 1,
                side: Side::Old,
                character: 1,
            },
        );
        assert_eq!(selection.text(&document), "ld\nne");
    }

    #[test]
    fn line_and_shift_selection_keep_the_original_anchor() {
        let document = document(vec![row(
            RowKind::Context,
            "let café_name = 😀;",
            "let café_name = 😀;",
        )]);
        let mut selection = Selection::default();
        selection.begin(&document, 0, Pane::Old, point(0, 6), false, true);
        assert_eq!(selection.text(&document), "let café_name = 😀;");
        assert!(!selection.dragging);
        selection.begin(&document, 0, Pane::Old, point(0, 3), true, false);
        assert_eq!(selection.text(&document), "let");
        selection.extend(&document, point(0, usize::MAX));
        assert_eq!(selection.text(&document), "let café_name = 😀;");
    }

    #[test]
    fn full_source_is_copied_across_folded_rows_and_long_line_previews() {
        let long = "x".repeat(crate::highlight::MAX_PREVIEW_BYTES + 32);
        let mut rows: Vec<_> = (0..100)
            .map(|_| row(RowKind::Context, "folded", "folded"))
            .collect();
        rows.push(row(RowKind::Change, &long, "opposite"));
        let document = document(rows);
        assert!(
            document
                .side_lines
                .iter()
                .any(|line| matches!(line, super::super::DisplayLine::Fold { .. }))
        );
        let mut selection = Selection::default();
        selection.select_all(&document, 0, Pane::Old);
        assert_eq!(
            selection.text(&document),
            format!("{}{}", "folded\n".repeat(100), long)
        );
        selection.begin(&document, 0, Pane::Old, point(99, 3), false, false);
        selection.extend(&document, point(0, 2));
        assert_eq!(
            selection.text(&document),
            format!("lded\n{}fol", "folded\n".repeat(98))
        );
    }

    #[test]
    fn invalid_and_other_comparison_endpoints_do_not_extend_selection() {
        let mut document = document(vec![row(RowKind::Change, "old", "new")]);
        document.document.comparisons.push(Comparison {
            label: "Staged".into(),
            patch: String::new(),
            summary: None,
            rows: vec![row(RowKind::Change, "other old", "other new")],
        });
        let document = PreparedDocument::new(document.document, &CancelToken::default());
        let mut selection = Selection::default();
        selection.begin(&document, 0, Pane::Old, point(0, 0), false, false);
        selection.extend(&document, point(0, 3));
        selection.extend(&document, point(1, 5));
        selection.extend(&document, point(usize::MAX, 5));
        assert_eq!(selection.text(&document), "old");
        selection.select_all(&document, 1, Pane::New);
        assert_eq!(selection.text(&document), "other new");
    }

    #[test]
    fn select_all_includes_source_beyond_the_rendering_limit() {
        let rows = (0..100_005)
            .map(|_| row(RowKind::Addition, "", "source"))
            .collect();
        let document = document(rows);
        assert!(matches!(
            document.side_lines.last(),
            Some(super::super::DisplayLine::Limit { .. })
        ));
        let mut selection = Selection::default();
        selection.select_all(&document, 0, Pane::New);
        assert_eq!(selection.text(&document).lines().count(), 100_005);
        assert_eq!(selection.range.unwrap().1.row, 100_004);
    }

    #[test]
    fn empty_or_summary_only_comparisons_have_no_selected_source() {
        let document = document(Vec::new());
        let mut selection = Selection::default();
        selection.select_all(&document, 0, Pane::Old);
        assert_eq!(selection.text(&document), "");
        assert!(selection.range.is_none());
        selection.select_all(&document, usize::MAX, Pane::Unified);
        assert_eq!(selection.text(&document), "");
        assert!(selection.range.is_none());
    }

    #[test]
    fn indexed_full_file_selection_clamps_unicode_offsets_and_copies_original_text() {
        let long = "é\t漢😀\u{301}".repeat(1001);
        let count = long.chars().count();
        let document = PreparedDocument::from_text(
            "long.txt".into(),
            format!("{long}\n{long}"),
            &CancelToken::default(),
        );
        let mut selection = Selection::default();
        selection.begin(
            &document,
            0,
            Pane::Unified,
            point(1, usize::MAX),
            false,
            false,
        );
        selection.extend(&document, point(0, count - 3));
        assert_eq!(selection.range.unwrap().0.character, count);
        assert_eq!(selection.text(&document), format!("漢😀\u{301}\n{long}"));
        assert_eq!(
            selection.range_for(0, Pane::Unified, point(0, 0), &long, Some(count)),
            Some(count - 3..count)
        );
        selection.begin(
            &document,
            0,
            Pane::Unified,
            point(0, count - 1),
            false,
            true,
        );
        assert_eq!(selection.text(&document), long);
        selection.select_all(&document, 0, Pane::Unified);
        assert_eq!(selection.text(&document), format!("{long}\n{long}"));
    }
}

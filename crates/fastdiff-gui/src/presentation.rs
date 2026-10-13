use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VisibleRow {
    Source(usize),
    Fold { start: usize, end: usize },
}

pub fn visible_rows(context: &[bool], expanded: &HashSet<usize>) -> Vec<VisibleRow> {
    let mut visible = Vec::with_capacity(context.len());
    let mut cursor = 0;
    while cursor < context.len() {
        if !context[cursor] {
            visible.push(VisibleRow::Source(cursor));
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < context.len() && context[cursor] {
            cursor += 1;
        }
        let end = cursor;
        if end - start <= 8 || expanded.contains(&(start + 3)) {
            visible.extend((start..end).map(VisibleRow::Source));
        } else {
            visible.extend((start..start + 3).map(VisibleRow::Source));
            visible.push(VisibleRow::Fold {
                start: start + 3,
                end: end - 3,
            });
            visible.extend((end - 3..end).map(VisibleRow::Source));
        }
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_collapses_middle_but_keeps_edges_and_can_expand() {
        let context = vec![
            false, true, true, true, true, true, true, true, true, true, true, false,
        ];
        assert_eq!(
            visible_rows(&context, &HashSet::new()),
            vec![
                VisibleRow::Source(0),
                VisibleRow::Source(1),
                VisibleRow::Source(2),
                VisibleRow::Source(3),
                VisibleRow::Fold { start: 4, end: 8 },
                VisibleRow::Source(8),
                VisibleRow::Source(9),
                VisibleRow::Source(10),
                VisibleRow::Source(11)
            ]
        );
        assert_eq!(
            visible_rows(&context, &HashSet::from([4])).len(),
            context.len()
        );
    }
    #[test]
    fn short_context_remains_visible() {
        assert_eq!(
            visible_rows(&[true, true, false], &HashSet::new()),
            vec![
                VisibleRow::Source(0),
                VisibleRow::Source(1),
                VisibleRow::Source(2)
            ]
        );
    }
}

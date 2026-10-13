use crate::{Comparison, DiffRow, RowKind};

pub(crate) fn summary(label: &str, message: impl Into<String>) -> Comparison {
    Comparison {
        label: label.into(),
        patch: String::new(),
        rows: Vec::new(),
        summary: Some(message.into()),
    }
}

pub(crate) fn parse(label: &str, patch: String, limit: usize) -> Comparison {
    if patch
        .lines()
        .any(|line| line.starts_with("Binary files ") || line == "GIT binary patch")
    {
        return Comparison {
            label: label.into(),
            patch,
            rows: Vec::new(),
            summary: Some("Binary file changed; text preview unavailable.".into()),
        };
    }
    let mut parser = Parser {
        rows: Vec::new(),
        deleted: Vec::new(),
        added: Vec::new(),
        old: 0,
        new: 0,
        in_hunk: false,
        limit,
        used_rows: 0,
        truncated: false,
    };
    for line in patch.lines() {
        parser.line(line);
        if parser.truncated {
            break;
        }
    }
    parser.flush();
    let summary = if parser.truncated {
        Some(format!(
            "Diff exceeds the {limit} row rendering limit; preview truncated."
        ))
    } else if parser.rows.is_empty() {
        Some("No text changes (the file may have been renamed or refreshed).".into())
    } else {
        None
    };
    Comparison {
        label: label.into(),
        patch,
        rows: parser.rows,
        summary,
    }
}

struct Parser {
    rows: Vec<DiffRow>,
    deleted: Vec<(usize, String)>,
    added: Vec<(usize, String)>,
    old: usize,
    new: usize,
    in_hunk: bool,
    limit: usize,
    used_rows: usize,
    truncated: bool,
}

impl Parser {
    fn push(&mut self, row: DiffRow) {
        let weight = rendered_rows(&row);
        if self.used_rows.saturating_add(weight) <= self.limit {
            self.rows.push(row);
            self.used_rows += weight;
        } else {
            self.truncated = true;
        }
    }

    fn line(&mut self, line: &str) {
        if line.starts_with("@@ ") {
            self.flush();
            let mut parts = line.split_whitespace();
            self.old = parts.nth(1).and_then(number).unwrap_or(0);
            self.new = parts.next().and_then(number).unwrap_or(0);
            self.in_hunk = true;
            self.push(DiffRow {
                kind: RowKind::Header,
                old_number: None,
                new_number: None,
                left: line.into(),
                right: line.into(),
            });
            return;
        }
        if !self.in_hunk {
            return;
        }
        match line.as_bytes().first() {
            Some(b'-') => {
                if self.deleted.len() >= self.limit {
                    self.truncated = true;
                    return;
                }
                self.deleted.push((self.old, line[1..].into()));
                self.old += 1;
            }
            Some(b'+') => {
                if self.added.len() >= self.limit {
                    self.truncated = true;
                    return;
                }
                self.added.push((self.new, line[1..].into()));
                self.new += 1;
            }
            Some(b' ') => {
                self.flush();
                self.push(DiffRow {
                    kind: RowKind::Context,
                    old_number: Some(self.old),
                    new_number: Some(self.new),
                    left: line[1..].into(),
                    right: line[1..].into(),
                });
                self.old += 1;
                self.new += 1;
            }
            _ => {}
        }
    }

    fn flush(&mut self) {
        let deleted = std::mem::take(&mut self.deleted);
        let added = std::mem::take(&mut self.added);
        for i in 0..deleted.len().max(added.len()) {
            let old = deleted.get(i);
            let new = added.get(i);
            let kind = match (old.is_some(), new.is_some()) {
                (true, true) => RowKind::Change,
                (true, false) => RowKind::Deletion,
                _ => RowKind::Addition,
            };
            self.push(DiffRow {
                kind,
                old_number: old.map(|x| x.0),
                new_number: new.map(|x| x.0),
                left: old.map(|x| x.1.clone()).unwrap_or_default(),
                right: new.map(|x| x.1.clone()).unwrap_or_default(),
            });
            if self.truncated {
                break;
            }
        }
    }
}

pub(crate) fn rendered_rows(row: &DiffRow) -> usize {
    if row.kind == RowKind::Change { 2 } else { 1 }
}

fn number(value: &str) -> Option<usize> {
    value.get(1..)?.split(',').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_addition_deletion_and_no_newline_marker_keep_numbers() {
        let addition = parse(
            "Untracked",
            "@@ -0,0 +1,2 @@\n+one\n+two\n\\ No newline at end of file\n".into(),
            100,
        );
        assert_eq!(addition.rows[1].new_number, Some(1));
        assert_eq!(addition.rows[2].right, "two");
        let deletion = parse("Unstaged", "@@ -8,1 +8,0 @@\n-gone\n".into(), 100);
        assert_eq!(deletion.rows[1].old_number, Some(8));
        assert_eq!(deletion.rows[1].kind, RowKind::Deletion);
    }

    #[test]
    fn multiple_hunks_reset_numbers_and_header_metadata_is_not_source() {
        let document = parse(
            "Unstaged",
            "--- a/f\n+++ b/f\n@@ -2 +2 @@ function\n-a\n+b\n@@ -42 +49 @@\n context\n".into(),
            100,
        );
        assert_eq!(document.rows.len(), 4);
        assert_eq!(document.rows[1].kind, RowKind::Change);
        assert_eq!(document.rows[3].old_number, Some(42));
        assert_eq!(document.rows[3].new_number, Some(49));
    }

    #[test]
    fn row_limit_is_applied_to_paired_rows_and_context() {
        let document = parse(
            "Unstaged",
            "@@ -1,3 +1,3 @@\n-a\n-b\n-c\n+x\n+y\n+z\n".into(),
            3,
        );
        assert_eq!(
            document.rows.len(),
            2,
            "a paired change renders two unified lines"
        );
        assert!(document.summary.unwrap().contains("truncated"));
        let document = parse("Unstaged", "@@ -1 +1 @@\n unchanged\n".into(), 1);
        assert_eq!(document.rows.len(), 1);
        assert!(document.summary.is_some());
    }

    #[test]
    fn empty_and_binary_patches_have_summaries() {
        assert!(parse("Staged", String::new(), 10).summary.is_some());
        assert!(
            parse("Staged", "GIT binary patch\n".into(), 10)
                .summary
                .unwrap()
                .contains("Binary")
        );
        assert!(number("-invalid").is_none());
        assert!(number("").is_none());
    }
}

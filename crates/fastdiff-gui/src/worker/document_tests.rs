//! `PreparedDocument::from_text` must keep producing exactly what the general diff constructor
//! followed by the full-document overwrite produced.
use super::*;

/// The construction `from_text` used before it got a dedicated path, kept as the oracle.
fn legacy_from_text(text: &str, cancel: &CancelToken) -> PreparedDocument {
    let rows = text
        .lines()
        .enumerate()
        .map(|(index, line)| fastdiff_core::DiffRow {
            kind: fastdiff_core::RowKind::Context,
            old_number: None,
            new_number: Some(index + 1),
            left: String::new(),
            right: line.to_owned(),
        })
        .collect();
    let mut prepared = PreparedDocument::new(
        DiffDocument {
            comparisons: vec![fastdiff_core::Comparison {
                label: "Full file".into(),
                patch: String::new(),
                summary: None,
                rows,
            }],
        },
        cancel,
    );
    prepared.is_document = true;
    let mut checkpoints = Vec::new();
    prepared.right_columns = prepared.document.comparisons[0]
        .rows
        .iter()
        .enumerate()
        .map(|(row, source)| {
            let indexed = source.right.len() > LONG_LINE_BYTES;
            let mut columns = 0;
            let mut characters = 0;
            for (byte, character) in source.right.char_indices() {
                if characters % CHECKPOINT_CHARACTERS == 0 {
                    if cancel.is_cancelled() {
                        return 0;
                    }
                    if indexed {
                        checkpoints.push(LineCheckpoint {
                            row,
                            byte,
                            character: characters,
                            columns,
                        });
                    }
                }
                columns += character_columns(character);
                characters += 1;
            }
            if indexed {
                checkpoints.push(LineCheckpoint {
                    row,
                    byte: source.right.len(),
                    character: characters,
                    columns,
                });
            }
            columns
        })
        .max()
        .unwrap_or(0);
    if !cancel.is_cancelled() {
        prepared.long_line_checkpoints = checkpoints;
    }
    prepared.max_columns = prepared.right_columns;
    prepared.context.fill(false);
    prepared.side_lines = make_lines(&prepared, true, &Default::default()).into();
    prepared.unified_lines = Arc::clone(&prepared.side_lines);
    prepared
}

type Rows = Vec<(usize, usize, String, String)>;
type Checkpoints = Vec<(usize, usize, usize, usize)>;

fn rows(document: &PreparedDocument) -> Rows {
    document
        .rows
        .iter()
        .map(|row| {
            (
                row.comparison,
                row.index,
                format!("{:?}", row.left),
                format!("{:?}", row.right),
            )
        })
        .collect()
}

fn checkpoints(document: &PreparedDocument) -> Checkpoints {
    document
        .long_line_checkpoints
        .iter()
        .map(|c| (c.row, c.byte, c.character, c.columns))
        .collect()
}

fn assert_same(text: &str, cancel: &CancelToken) {
    let expected = legacy_from_text(text, cancel);
    let actual = PreparedDocument::from_text("path".into(), text.to_owned(), cancel);
    assert!(actual.is_document, "{text:?}");
    assert_eq!(actual.document, expected.document, "{text:?}");
    assert_eq!(rows(&actual), rows(&expected), "{text:?}");
    assert_eq!(actual.context, expected.context, "{text:?}");
    assert_eq!(actual.max_columns, expected.max_columns, "{text:?}");
    assert_eq!(actual.left_columns, expected.left_columns, "{text:?}");
    assert_eq!(actual.right_columns, expected.right_columns, "{text:?}");
    assert_eq!(checkpoints(&actual), checkpoints(&expected), "{text:?}");
    assert_eq!(
        format!("{:?}", actual.side_lines),
        format!("{:?}", expected.side_lines),
        "{text:?}"
    );
    assert_eq!(
        format!("{:?}", actual.unified_lines),
        format!("{:?}", expected.unified_lines),
        "{text:?}"
    );
    assert!(Arc::ptr_eq(&actual.side_lines, &actual.unified_lines));
}

fn samples() -> Vec<String> {
    let wide_boundary = format!("{}日{}", "x".repeat(255), "y".repeat(5000));
    vec![
        String::new(),
        "\n".into(),
        "\n\n\n".into(),
        "single".into(),
        "trailing newline\n".into(),
        "no trailing newline\nsecond".into(),
        "crlf\r\nlines\r\n\r\nand a blank one\r\n".into(),
        "lone\rcarriage return\n".into(),
        "\t\tindented\twith tabs\n".into(),
        "日本語の行\nwide ｆｕｌｌ ｗｉｄｔｈ\n".into(),
        "zero\u{200b}width and e\u{301} combining\n".into(),
        "// a comment line\n# another\n-- sql\nplain code(\"string\", 42)\n".into(),
        "é".repeat(5000),
        format!("{}\nshort\n{}\n", "x".repeat(5000), "y".repeat(9000)),
        wide_boundary,
        format!("{}\n", "é".repeat(4096)),
        "line\n".repeat(2000),
        format!("{}\n", "x".repeat(LONG_LINE_BYTES)),
        format!("{}\n", "x".repeat(LONG_LINE_BYTES + 1)),
    ]
}

#[test]
fn full_document_matches_the_general_constructor_output() {
    let token = CancelToken::default();
    for text in samples() {
        assert_same(&text, &token);
    }
}

#[test]
fn full_document_matches_the_general_constructor_when_cancelled() {
    let token = CancelToken::default();
    token.cancel();
    for text in samples() {
        assert_same(&text, &token);
    }
}

#[test]
fn full_document_reports_columns_for_the_widest_line_only() {
    let token = CancelToken::default();
    let document =
        PreparedDocument::from_text("path".into(), "ab\n日本語日本語\n\tx\n".into(), &token);
    assert_eq!(document.left_columns, 0);
    assert_eq!(document.right_columns, 12);
    assert_eq!(document.max_columns, 12);
    assert_eq!(document.rows.len(), 3);
    assert!(document.context.iter().all(|context| !context));
}

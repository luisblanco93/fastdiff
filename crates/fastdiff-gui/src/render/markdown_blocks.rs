//! Splits Markdown into prose and top-level tables so each table can scroll sideways on its own.
//!
//! egui_commonmark draws a table as an unbounded grid, so a wide table runs past the reading
//! column. Drawing it in a separate horizontal scroll area needs the table as its own document.

use pulldown_cmark::{Event, Options, Parser, Tag};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockKind {
    Prose,
    Table,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarkdownBlock {
    pub(crate) kind: BlockKind,
    pub(crate) text: String,
}

/// Same extensions egui_commonmark enables, so the split agrees with how blocks are drawn.
fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
}

/// A link reference definition: its normalized label and where it sits in the source.
struct Definition {
    label: String,
    span: Range<usize>,
}

/// Byte ranges of tables that are not nested inside another block, plus every link reference
/// definition.
fn scan(text: &str) -> (Vec<Range<usize>>, Vec<Definition>) {
    let mut parser = Parser::new_ext(text, parser_options()).into_offset_iter();
    let mut tables = Vec::new();
    let mut depth = 0usize;
    for (event, range) in parser.by_ref() {
        match event {
            Event::Start(tag) => {
                if depth == 0 && matches!(tag, Tag::Table(_)) {
                    tables.push(range);
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let mut definitions = parser
        .reference_definitions()
        .iter()
        .map(|(label, definition)| Definition {
            label: label.to_lowercase(),
            span: definition.span.clone(),
        })
        .collect::<Vec<_>>();
    definitions.sort_by_key(|definition| definition.span.start);
    (tables, definitions)
}

/// Source of the definitions a block refers to, as `[label]`, that the block does not already
/// contain. Copying only used definitions keeps the cost proportional to the links in each block.
fn used_definitions(text: &str, block: &Range<usize>, definitions: &[Definition]) -> String {
    let lowered = text[block.clone()].to_lowercase();
    definitions
        .iter()
        .filter(|definition| {
            !(block.start <= definition.span.start && definition.span.end <= block.end)
                && lowered.contains(&format!("[{}]", definition.label))
        })
        .filter_map(|definition| text.get(definition.span.clone()))
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Returns the document as prose and table blocks in order. Link reference definitions that a
/// block uses are copied into it so reference links keep resolving after the split.
pub(crate) fn split_blocks(text: &str) -> Vec<MarkdownBlock> {
    let (tables, definitions) = scan(text);
    if tables.is_empty() {
        return if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![MarkdownBlock {
                kind: BlockKind::Prose,
                text: text.to_owned(),
            }]
        };
    }
    let mut blocks = Vec::new();
    let mut push = |kind, range: Range<usize>| {
        if text[range.clone()].trim().is_empty() {
            return;
        }
        let mut block = text[range.clone()].to_owned();
        let copied = used_definitions(text, &range, &definitions);
        if !copied.is_empty() {
            block.push_str("\n\n");
            block.push_str(&copied);
        }
        blocks.push(MarkdownBlock { kind, text: block });
    };
    let mut cursor = 0;
    for table in &tables {
        push(BlockKind::Prose, cursor..table.start);
        push(BlockKind::Table, table.clone());
        cursor = table.end;
    }
    push(BlockKind::Prose, cursor..text.len());
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(blocks: &[MarkdownBlock]) -> Vec<BlockKind> {
        blocks.iter().map(|block| block.kind).collect()
    }

    const TABLE: &str = "| a | b |\n|---|---|\n| 1 | 2 |\n";

    #[test]
    fn a_document_without_tables_is_one_unchanged_prose_block() {
        let text = "# Title\n\nbody [x]\n\n[x]: https://a.io\n";
        assert_eq!(
            split_blocks(text),
            vec![MarkdownBlock {
                kind: BlockKind::Prose,
                text: text.into()
            }]
        );
    }

    #[test]
    fn an_empty_or_blank_document_has_no_blocks() {
        assert!(split_blocks("").is_empty());
        assert!(split_blocks(" \n\n").is_empty());
    }

    #[test]
    fn a_table_between_prose_gives_prose_table_prose() {
        let text = format!("before\n\n{TABLE}\nafter\n");
        let blocks = split_blocks(&text);
        assert_eq!(
            kinds(&blocks),
            [BlockKind::Prose, BlockKind::Table, BlockKind::Prose]
        );
        assert_eq!(blocks[0].text.trim(), "before");
        assert_eq!(blocks[1].text, TABLE);
        assert_eq!(blocks[2].text.trim(), "after");
    }

    #[test]
    fn a_table_at_the_start_or_end_has_no_empty_prose_neighbour() {
        assert_eq!(kinds(&split_blocks(TABLE)), [BlockKind::Table]);
        let blocks = split_blocks(&format!("{TABLE}\ntail"));
        assert_eq!(kinds(&blocks), [BlockKind::Table, BlockKind::Prose]);
        let blocks = split_blocks(&format!("head\n\n{TABLE}"));
        assert_eq!(kinds(&blocks), [BlockKind::Prose, BlockKind::Table]);
    }

    #[test]
    fn consecutive_tables_become_separate_blocks() {
        let text = format!("{TABLE}\n{TABLE}");
        let blocks = split_blocks(&text);
        assert_eq!(kinds(&blocks), [BlockKind::Table, BlockKind::Table]);
    }

    #[test]
    fn a_table_inside_a_list_or_quote_stays_in_the_prose() {
        for text in [
            "- item\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |\n",
            "> | a | b |\n> |---|---|\n> | 1 | 2 |\n",
        ] {
            let blocks = split_blocks(text);
            assert_eq!(kinds(&blocks), [BlockKind::Prose], "{text}");
            assert_eq!(blocks[0].text, text);
        }
    }

    #[test]
    fn a_fenced_table_is_code_not_a_table() {
        let text = format!("```\n{TABLE}```\n");
        assert_eq!(kinds(&split_blocks(&text)), [BlockKind::Prose]);
    }

    #[test]
    fn a_block_gets_only_the_definitions_it_uses_and_lacks() {
        let text = format!(
            "see [x] and [Y][]\n\n{TABLE}\n[x]: https://a.io\n[y]: https://b.io\n[z]: https://c.io\n"
        );
        let blocks = split_blocks(&text);
        assert_eq!(blocks.len(), 3);
        // The first block uses x and y and contains neither definition.
        assert!(blocks[0].text.contains("[x]: https://a.io"));
        assert!(blocks[0].text.contains("[y]: https://b.io"));
        assert!(!blocks[0].text.contains("[z]:"));
        // The table uses no links, so it stays exactly the table.
        assert_eq!(blocks[1].text, TABLE);
        // The last block holds all three definitions itself and gets no copies.
        assert_eq!(blocks[2].text.matches("[x]: https://a.io").count(), 1);
    }

    #[test]
    fn many_definitions_and_tables_do_not_multiply_the_text() {
        let definitions = (0..500)
            .map(|n| format!("[d{n}]: https://a.io/{n}\n"))
            .collect::<String>();
        let tables = (0..200).map(|_| format!("{TABLE}\n")).collect::<String>();
        let text = format!("{tables}{definitions}");
        let total: usize = split_blocks(&text).iter().map(|b| b.text.len()).sum();
        assert!(total <= text.len() + 64, "{total} vs {}", text.len());
    }
}

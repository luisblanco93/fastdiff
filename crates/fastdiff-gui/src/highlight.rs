//! Small bounded source lexer. Tokenization runs on the repository worker.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
}

#[derive(Clone, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

pub const MAX_PREVIEW_BYTES: usize = 4096;
pub const PREVIEW_SUFFIX: &str = " … [long line; copy source for full text]";

pub fn preview_len(text: &str) -> usize {
    let mut end = text.len().min(MAX_PREVIEW_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

pub fn highlight(text: &str) -> Vec<Span> {
    let text = &text[..preview_len(text)];
    let mut spans = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let tail = &text[cursor..];
        let character = tail.chars().next().unwrap_or(' ');
        if tail.starts_with("//") || tail.starts_with('#') || tail.starts_with("-- ") {
            spans.push(Span {
                start: cursor,
                end: text.len(),
                kind: TokenKind::Comment,
            });
            break;
        }
        if character == '\'' || character == '"' || character == '`' {
            let start = cursor;
            cursor += character.len_utf8();
            let mut escaped = false;
            for ch in text[cursor..].chars() {
                cursor += ch.len_utf8();
                if ch == character && !escaped {
                    break;
                }
                escaped = ch == '\\' && !escaped;
            }
            spans.push(Span {
                start,
                end: cursor,
                kind: TokenKind::String,
            });
        } else if character.is_alphanumeric() || character == '_' {
            let start = cursor;
            cursor += tail
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '.')
                .map(char::len_utf8)
                .sum::<usize>();
            let word = &text[start..cursor];
            let kind = if character.is_ascii_digit() {
                Some(TokenKind::Number)
            } else if is_keyword(word) {
                Some(TokenKind::Keyword)
            } else {
                None
            };
            if let Some(kind) = kind {
                spans.push(Span {
                    start,
                    end: cursor,
                    kind,
                });
            }
        } else {
            cursor += character.len_utf8();
        }
    }
    spans
}

fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "fn" | "pub"
            | "let"
            | "mut"
            | "impl"
            | "struct"
            | "enum"
            | "use"
            | "mod"
            | "self"
            | "Self"
            | "match"
            | "if"
            | "else"
            | "return"
            | "for"
            | "while"
            | "loop"
            | "in"
            | "true"
            | "false"
            | "None"
            | "Some"
            | "const"
            | "class"
            | "function"
            | "export"
            | "import"
            | "from"
            | "def"
            | "async"
            | "await"
            | "try"
            | "catch"
            | "throw"
            | "new"
            | "null"
            | "void"
            | "static"
            | "int"
            | "bool"
            | "package"
            | "type"
            | "interface"
            | "extends"
            | "yield"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokenizes_keywords_strings_numbers_and_comments_without_overlapping() {
        let text = "let value = \"héllo\"; // comment 42";
        let spans = highlight(text);
        assert_eq!(
            spans.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![TokenKind::Keyword, TokenKind::String, TokenKind::Comment]
        );
        assert!(spans.windows(2).all(|w| w[0].end <= w[1].start));
        assert!(
            spans
                .iter()
                .all(|s| text.is_char_boundary(s.start) && text.is_char_boundary(s.end))
        );
        assert_eq!(highlight("42")[0].kind, TokenKind::Number);
    }
    #[test]
    fn huge_unicode_lines_have_a_bounded_valid_preview() {
        let text = "é".repeat(10000);
        assert!(preview_len(&text) <= MAX_PREVIEW_BYTES);
        assert!(text.is_char_boundary(preview_len(&text)));
        assert!(highlight(&text).iter().all(|s| s.end <= MAX_PREVIEW_BYTES));
    }
}

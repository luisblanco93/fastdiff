//! Small bounded source lexer. Tokenization runs on the repository worker.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Type,
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
            } else if is_type(word) {
                Some(TokenKind::Type)
            } else {
                None
            };
            if let Some(kind) = kind {
                spans.push(Span {
                    start,
                    end: cursor,
                    kind,
                });
            } else if let Some(offset) = function_offset(word, &text[cursor..]) {
                spans.push(Span {
                    start: start + offset,
                    end: cursor,
                    kind: TokenKind::Function,
                });
            }
        } else {
            cursor += character.len_utf8();
        }
    }
    spans
}

/// A type name has no `.`, starts with an ASCII capital and mixes in lowercase, so `HashMap`
/// and `Ok` qualify while `MAX_X`, `HTTP` and `T` stay plain.
fn is_type(word: &str) -> bool {
    word.starts_with(|ch: char| ch.is_ascii_uppercase())
        && !word.contains('.')
        && word.chars().any(|ch| ch.is_lowercase())
}

/// Start of the called name inside `word` when `after` opens a call, or a Rust macro
/// invocation (`name!(`, `name![`, `name!{`). Only the last segment of a dotted word is the
/// callee, so `theme.diff.add(` colors just `add`.
fn function_offset(word: &str, after: &str) -> Option<usize> {
    let opens_call = after.starts_with('(')
        || after
            .strip_prefix('!')
            .is_some_and(|rest| rest.starts_with(['(', '[', '{']));
    let offset = word.rfind('.').map_or(0, |dot| dot + 1);
    let name = &word[offset..];
    (opens_call && !name.is_empty() && !name.starts_with(|ch: char| ch.is_ascii_digit()))
        .then_some(offset)
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
    fn kinds(text: &str) -> Vec<(&str, TokenKind)> {
        highlight(text)
            .iter()
            .map(|span| (&text[span.start..span.end], span.kind))
            .collect()
    }
    #[test]
    fn tokenizes_functions_types_and_their_precedence() {
        assert_eq!(
            kinds("run(1)"),
            [("run", TokenKind::Function), ("1", TokenKind::Number)]
        );
        assert_eq!(kinds("println!(x)"), [("println", TokenKind::Function)]);
        assert_eq!(
            kinds("fn open(x)"),
            [("fn", TokenKind::Keyword), ("open", TokenKind::Function)]
        );
        assert_eq!(kinds("theme.diff.add(x)"), [("add", TokenKind::Function)]);
        for plain in ["value (x)", "a != b", "theme.diff.add", "self.0(x)"] {
            assert!(kinds(plain).is_empty(), "{plain}");
        }
        assert_eq!(kinds("HashMap<K>"), [("HashMap", TokenKind::Type)]);
        assert_eq!(
            kinds("Vec::with_capacity(x)"),
            [
                ("Vec", TokenKind::Type),
                ("with_capacity", TokenKind::Function)
            ]
        );
        assert_eq!(kinds("Some(x)"), [("Some", TokenKind::Keyword)]);
        assert_eq!(kinds("1.5"), [("1.5", TokenKind::Number)]);
        for plain in ["MAX_X", "HTTP", "T", "Foo.bar"] {
            assert!(kinds(plain).is_empty(), "{plain}");
        }
    }
    #[test]
    fn huge_unicode_lines_have_a_bounded_valid_preview() {
        let text = "é".repeat(10000);
        assert!(preview_len(&text) <= MAX_PREVIEW_BYTES);
        assert!(text.is_char_boundary(preview_len(&text)));
        assert!(highlight(&text).iter().all(|s| s.end <= MAX_PREVIEW_BYTES));
    }
}

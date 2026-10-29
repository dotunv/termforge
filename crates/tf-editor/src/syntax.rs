use std::ops::Range;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Shell,
    Json,
    Toml,
    Yaml,
    Markdown,
    CLike,
    Css,
    PlainText,
}

impl Language {
    pub fn from_path(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        match extension.to_ascii_lowercase().as_str() {
            "rs" => Self::Rust,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "ts" | "tsx" | "mts" | "cts" => Self::TypeScript,
            "py" | "pyi" => Self::Python,
            "sh" | "bash" | "zsh" | "fish" | "ps1" => Self::Shell,
            "json" | "jsonc" => Self::Json,
            "toml" => Self::Toml,
            "yaml" | "yml" => Self::Yaml,
            "md" | "mdx" => Self::Markdown,
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "java" | "cs" | "go" | "swift" | "kt"
            | "kts" => Self::CLike,
            "css" | "scss" | "sass" | "less" => Self::Css,
            _ if matches!(name, "Dockerfile" | "Makefile") => Self::Shell,
            _ => Self::PlainText,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Comment,
    String,
    Number,
    Keyword,
    Type,
    Heading,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

/// A deliberately dependency-free baseline highlighter. It remains available
/// when no parser or language server is installed and only emits valid UTF-8
/// byte ranges. Rich parser-backed highlighting can replace it behind the same
/// output type.
pub fn highlight(text: &str, language: Language) -> Vec<HighlightSpan> {
    if language == Language::PlainText {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if is_line_comment(bytes, index, language) {
            let end = text[index..]
                .find('\n')
                .map_or(text.len(), |offset| index + offset);
            spans.push(span(index..end, HighlightKind::Comment));
            index = end;
            continue;
        }
        if supports_block_comments(language) && bytes[index..].starts_with(b"/*") {
            let end = text[index + 2..]
                .find("*/")
                .map_or(text.len(), |offset| index + 2 + offset + 2);
            spans.push(span(index..end, HighlightKind::Comment));
            index = end;
            continue;
        }
        if language == Language::Markdown && at_line_start(bytes, index) && bytes[index] == b'#' {
            let end = text[index..]
                .find('\n')
                .map_or(text.len(), |offset| index + offset);
            spans.push(span(index..end, HighlightKind::Heading));
            index = end;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"' | b'`') {
            let quote = bytes[index];
            let mut end = index + 1;
            while end < bytes.len() {
                if bytes[end] == b'\\' {
                    end = (end + 2).min(bytes.len());
                } else if bytes[end] == quote {
                    end += 1;
                    break;
                } else if bytes[end] == b'\n' && quote != b'`' {
                    break;
                } else {
                    end += char_width(text, end);
                }
            }
            spans.push(span(index..end, HighlightKind::String));
            index = end;
            continue;
        }
        if bytes[index].is_ascii_digit() {
            let mut end = index + 1;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric()
                    || matches!(bytes[end], b'.' | b'_' | b'x' | b'o' | b'b'))
            {
                end += 1;
            }
            spans.push(span(index..end, HighlightKind::Number));
            index = end;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or_default();
        if is_identifier_start(character) {
            let mut end = index + character.len_utf8();
            while end < bytes.len() {
                let next = text[end..].chars().next().unwrap_or_default();
                if !is_identifier_continue(next) {
                    break;
                }
                end += next.len_utf8();
            }
            let word = &text[index..end];
            if is_keyword(language, word) {
                spans.push(span(index..end, HighlightKind::Keyword));
            } else if is_type_word(language, word) {
                spans.push(span(index..end, HighlightKind::Type));
            }
            index = end;
            continue;
        }
        index += character.len_utf8().max(1);
    }
    spans
}

fn span(range: Range<usize>, kind: HighlightKind) -> HighlightSpan {
    HighlightSpan { range, kind }
}

fn char_width(text: &str, index: usize) -> usize {
    text[index..].chars().next().map_or(1, char::len_utf8)
}

fn at_line_start(bytes: &[u8], index: usize) -> bool {
    index == 0 || bytes.get(index.wrapping_sub(1)) == Some(&b'\n')
}

fn is_line_comment(bytes: &[u8], index: usize, language: Language) -> bool {
    if bytes[index..].starts_with(b"//")
        && matches!(
            language,
            Language::Rust
                | Language::JavaScript
                | Language::TypeScript
                | Language::Json
                | Language::CLike
                | Language::Css
        )
    {
        return true;
    }
    bytes[index] == b'#'
        && matches!(
            language,
            Language::Python | Language::Shell | Language::Toml | Language::Yaml
        )
}

fn supports_block_comments(language: Language) -> bool {
    matches!(
        language,
        Language::Rust
            | Language::JavaScript
            | Language::TypeScript
            | Language::Json
            | Language::CLike
            | Language::Css
    )
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_ascii_digit()
}

fn is_keyword(language: Language, word: &str) -> bool {
    let common = matches!(
        word,
        "true"
            | "false"
            | "null"
            | "return"
            | "if"
            | "else"
            | "for"
            | "while"
            | "break"
            | "continue"
            | "match"
            | "switch"
            | "case"
            | "default"
            | "async"
            | "await"
            | "import"
            | "export"
            | "from"
            | "as"
    );
    common
        || match language {
            Language::Rust => matches!(
                word,
                "fn" | "let"
                    | "mut"
                    | "pub"
                    | "impl"
                    | "trait"
                    | "struct"
                    | "enum"
                    | "use"
                    | "mod"
                    | "crate"
                    | "self"
                    | "Self"
                    | "where"
                    | "move"
                    | "ref"
                    | "const"
                    | "static"
                    | "unsafe"
                    | "dyn"
            ),
            Language::JavaScript | Language::TypeScript => matches!(
                word,
                "const"
                    | "let"
                    | "var"
                    | "function"
                    | "class"
                    | "new"
                    | "this"
                    | "extends"
                    | "try"
                    | "catch"
                    | "finally"
                    | "throw"
                    | "typeof"
                    | "instanceof"
                    | "in"
                    | "of"
                    | "yield"
                    | "delete"
                    | "interface"
                    | "type"
                    | "namespace"
                    | "declare"
            ),
            Language::Python => matches!(
                word,
                "def"
                    | "class"
                    | "lambda"
                    | "with"
                    | "try"
                    | "except"
                    | "finally"
                    | "raise"
                    | "yield"
                    | "pass"
                    | "in"
                    | "is"
                    | "not"
                    | "and"
                    | "or"
                    | "None"
                    | "True"
                    | "False"
                    | "global"
                    | "nonlocal"
            ),
            Language::Shell => matches!(
                word,
                "then" | "fi" | "do" | "done" | "function" | "select" | "until" | "esac"
            ),
            Language::CLike => matches!(
                word,
                "class"
                    | "struct"
                    | "enum"
                    | "interface"
                    | "package"
                    | "public"
                    | "private"
                    | "protected"
                    | "static"
                    | "final"
                    | "const"
                    | "void"
                    | "new"
                    | "this"
                    | "try"
                    | "catch"
                    | "throw"
                    | "defer"
                    | "go"
                    | "func"
                    | "range"
            ),
            Language::Json => matches!(word, "true" | "false" | "null"),
            _ => false,
        }
}

fn is_type_word(language: Language, word: &str) -> bool {
    matches!(
        language,
        Language::Rust | Language::TypeScript | Language::CLike
    ) && matches!(
        word,
        "bool"
            | "char"
            | "str"
            | "string"
            | "String"
            | "int"
            | "float"
            | "double"
            | "usize"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "f32"
            | "f64"
            | "void"
            | "unknown"
            | "never"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_detection_covers_common_project_files() {
        assert_eq!(
            Language::from_path(Path::new("src/main.rs")),
            Language::Rust
        );
        assert_eq!(
            Language::from_path(Path::new("Dockerfile")),
            Language::Shell
        );
        assert_eq!(
            Language::from_path(Path::new("README.md")),
            Language::Markdown
        );
    }

    #[test]
    fn rust_highlights_are_ordered_valid_ranges() {
        let text = "pub fn main() { let name: String = \"🙂\"; // hello\n}";
        let spans = highlight(text, Language::Rust);
        assert!(spans
            .iter()
            .any(|span| span.kind == HighlightKind::Keyword && &text[span.range.clone()] == "pub"));
        assert!(spans.iter().any(
            |span| span.kind == HighlightKind::String && &text[span.range.clone()] == "\"🙂\""
        ));
        assert!(spans
            .iter()
            .any(|span| span.kind == HighlightKind::Comment
                && &text[span.range.clone()] == "// hello"));
        assert!(spans
            .windows(2)
            .all(|pair| pair[0].range.end <= pair[1].range.start));
        assert!(spans
            .iter()
            .all(|span| text.is_char_boundary(span.range.start)
                && text.is_char_boundary(span.range.end)));
    }

    #[test]
    fn markdown_headings_and_python_comments_are_recognized() {
        assert_eq!(
            highlight("# Heading\nbody", Language::Markdown)[0].kind,
            HighlightKind::Heading
        );
        assert_eq!(
            highlight("value = 42 # note", Language::Python)
                .last()
                .unwrap()
                .kind,
            HighlightKind::Comment
        );
    }
}

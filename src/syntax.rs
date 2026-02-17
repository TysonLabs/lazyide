use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxLang {
    Plain,
    Rust,
    Python,
    JsTs,
    Go,
    Php,
    Css,
    HtmlXml,
    Shell,
    Json,
    Markdown,
}
pub(crate) fn syntax_lang_for_path(path: Option<&Path>) -> SyntaxLang {
    let Some(path) = path else {
        return SyntaxLang::Plain;
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => SyntaxLang::Rust,
        "py" | "pyi" => SyntaxLang::Python,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" => SyntaxLang::JsTs,
        "go" => SyntaxLang::Go,
        "php" | "phtml" => SyntaxLang::Php,
        "css" | "scss" | "sass" | "less" => SyntaxLang::Css,
        "html" | "htm" | "xml" | "svg" | "xhtml" | "vue" | "svelte" | "astro" | "jsp" | "erb"
        | "hbs" | "ejs" => SyntaxLang::HtmlXml,
        "sh" | "bash" | "zsh" | "fish" | "ksh" => SyntaxLang::Shell,
        "json" | "jsonc" | "toml" | "yaml" | "yml" => SyntaxLang::Json,
        "md" | "markdown" => SyntaxLang::Markdown,
        _ => SyntaxLang::Plain,
    }
}

pub(crate) fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

pub(crate) fn keywords_for_lang(lang: SyntaxLang) -> &'static [&'static str] {
    match lang {
        SyntaxLang::Rust => &[
            "fn", "let", "mut", "impl", "trait", "struct", "enum", "match", "if", "else", "for",
            "while", "loop", "pub", "use", "mod", "crate", "self", "super", "return", "async",
            "await", "move", "const", "static", "where", "in", "break", "continue", "type", "dyn",
        ],
        SyntaxLang::Python => &[
            "def", "class", "if", "elif", "else", "for", "while", "try", "except", "return",
            "import", "from", "as", "with", "async", "await", "yield", "lambda", "pass", "None",
            "True", "False",
        ],
        SyntaxLang::JsTs => &[
            "function",
            "const",
            "let",
            "var",
            "class",
            "if",
            "else",
            "for",
            "while",
            "return",
            "import",
            "from",
            "export",
            "default",
            "async",
            "await",
            "try",
            "catch",
            "switch",
            "case",
            "break",
            "continue",
            "interface",
            "type",
            "extends",
            "implements",
        ],
        SyntaxLang::Go => &[
            "package",
            "import",
            "func",
            "var",
            "const",
            "type",
            "struct",
            "interface",
            "map",
            "chan",
            "go",
            "defer",
            "select",
            "if",
            "else",
            "switch",
            "case",
            "default",
            "for",
            "range",
            "return",
            "break",
            "continue",
            "fallthrough",
        ],
        SyntaxLang::Php => &[
            "function",
            "class",
            "interface",
            "trait",
            "public",
            "private",
            "protected",
            "static",
            "if",
            "else",
            "elseif",
            "switch",
            "case",
            "default",
            "for",
            "foreach",
            "while",
            "do",
            "return",
            "new",
            "use",
            "namespace",
            "try",
            "catch",
            "finally",
            "fn",
        ],
        SyntaxLang::Css => &[
            "@media",
            "@supports",
            "@keyframes",
            "display",
            "position",
            "color",
            "background",
            "border",
            "margin",
            "padding",
            "width",
            "height",
            "font",
            "grid",
            "flex",
        ],
        SyntaxLang::Shell => &[
            "if", "then", "else", "fi", "for", "do", "done", "while", "case", "esac", "function",
            "export", "local",
        ],
        SyntaxLang::HtmlXml | SyntaxLang::Json | SyntaxLang::Markdown | SyntaxLang::Plain => &[],
    }
}

pub(crate) fn comment_start_for_lang(lang: SyntaxLang) -> Option<&'static str> {
    match lang {
        SyntaxLang::Rust | SyntaxLang::JsTs | SyntaxLang::Go => Some("//"),
        SyntaxLang::Php | SyntaxLang::Css => Some("/*"),
        SyntaxLang::Python | SyntaxLang::Shell => Some("#"),
        SyntaxLang::HtmlXml | SyntaxLang::Json | SyntaxLang::Markdown | SyntaxLang::Plain => None,
    }
}

pub(crate) fn highlight_line(
    line: &str,
    lang: SyntaxLang,
    theme: &Theme,
    bracket_depth: u16,
    bracket_colors: &[Color; 3],
) -> Line<'static> {
    let base = Style::default().fg(theme.fg);
    if lang == SyntaxLang::Plain {
        return Line::from(vec![Span::styled(line.to_string(), base)]);
    }
    let keyword_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let string_style = Style::default().fg(theme.syntax_string);
    let number_style = Style::default().fg(theme.syntax_number);
    let comment_style = Style::default().fg(theme.comment);
    let heading_style = Style::default()
        .fg(theme.syntax_tag)
        .add_modifier(Modifier::BOLD);

    if lang == SyntaxLang::Markdown {
        if line.starts_with('#') {
            return Line::from(vec![Span::styled(line.to_string(), heading_style)]);
        }
        return Line::from(vec![Span::styled(line.to_string(), base)]);
    }
    if lang == SyntaxLang::HtmlXml {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut i = 0usize;
        let bytes = line.as_bytes();
        let tag_style = Style::default()
            .fg(theme.syntax_tag)
            .add_modifier(Modifier::BOLD);
        let attr_style = Style::default().fg(theme.syntax_attribute);
        while i < bytes.len() {
            if line[i..].starts_with("<!--") {
                spans.push(Span::styled(line[i..].to_string(), comment_style));
                break;
            }
            let ch = line[i..].chars().next().unwrap_or('\0');
            if ch == '<' {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    let c = line[i..].chars().next().unwrap_or('\0');
                    i += c.len_utf8();
                    if c == '>' {
                        break;
                    }
                }
                let tag = &line[start..i];
                let mut parts = tag.split_whitespace();
                if let Some(head) = parts.next() {
                    spans.push(Span::styled(head.to_string(), tag_style));
                    for part in parts {
                        spans.push(Span::raw(" ".to_string()));
                        if let Some(eq_idx) = part.find('=') {
                            let (k, v) = part.split_at(eq_idx);
                            spans.push(Span::styled(k.to_string(), attr_style));
                            spans.push(Span::raw(v.to_string()));
                        } else {
                            spans.push(Span::styled(part.to_string(), attr_style));
                        }
                    }
                } else {
                    spans.push(Span::styled(tag.to_string(), tag_style));
                }
                continue;
            }
            if ch == '"' || ch == '\'' {
                let quote = ch;
                let start = i;
                i += ch.len_utf8();
                while i < bytes.len() {
                    let c = line[i..].chars().next().unwrap_or('\0');
                    i += c.len_utf8();
                    if c == quote {
                        break;
                    }
                }
                spans.push(Span::styled(line[start..i].to_string(), string_style));
                continue;
            }
            spans.push(Span::styled(ch.to_string(), base));
            i += ch.len_utf8();
        }
        return Line::from(spans);
    }

    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut bd = bracket_depth;
    let is_block_comment_lang = matches!(lang, SyntaxLang::Php | SyntaxLang::Css);

    // Heuristic for middle lines of multiline block comments in CSS/PHP.
    let trimmed = line.trim_start();
    if is_block_comment_lang && (trimmed.starts_with('*') || trimmed.starts_with("*/")) {
        return Line::from(vec![Span::styled(line.to_string(), comment_style)]);
    }
    while i < bytes.len() {
        if let Some(comment) = comment_start_for_lang(lang) {
            if comment == "/*" && line[i..].starts_with("/*") {
                // Style only the block segment when it closes on this line.
                if let Some(close_rel) = line[i + 2..].find("*/") {
                    let end = i + 2 + close_rel + 2;
                    spans.push(Span::styled(line[i..end].to_string(), comment_style));
                    i = end;
                    continue;
                }
                spans.push(Span::styled(line[i..].to_string(), comment_style));
                break;
            } else if comment != "/*" && line[i..].starts_with(comment) {
                spans.push(Span::styled(line[i..].to_string(), comment_style));
                break;
            }
        }
        let ch = line[i..].chars().next().unwrap_or('\0');
        if ch == '"' || ch == '\'' {
            let quote = ch;
            let start = i;
            i += ch.len_utf8();
            while i < bytes.len() {
                let c = line[i..].chars().next().unwrap_or('\0');
                i += c.len_utf8();
                if c == '\\' && i < bytes.len() {
                    let escaped = line[i..].chars().next().unwrap_or('\0');
                    i += escaped.len_utf8();
                    continue;
                }
                if c == quote {
                    break;
                }
            }
            spans.push(Span::styled(line[start..i].to_string(), string_style));
            continue;
        }
        if ch.is_ascii_digit() {
            let start = i;
            i += ch.len_utf8();
            while i < bytes.len() {
                let c = line[i..].chars().next().unwrap_or('\0');
                if c.is_ascii_digit() || c == '_' || c == '.' {
                    i += c.len_utf8();
                } else {
                    break;
                }
            }
            spans.push(Span::styled(line[start..i].to_string(), number_style));
            continue;
        }
        if is_ident_char(ch) {
            let start = i;
            i += ch.len_utf8();
            while i < bytes.len() {
                let c = line[i..].chars().next().unwrap_or('\0');
                if is_ident_char(c) {
                    i += c.len_utf8();
                } else {
                    break;
                }
            }
            let token = &line[start..i];
            if keywords_for_lang(lang).contains(&token) {
                spans.push(Span::styled(token.to_string(), keyword_style));
            } else {
                spans.push(Span::styled(token.to_string(), base));
            }
            continue;
        }
        if ch == '{' || ch == '(' || ch == '[' {
            let color = bracket_colors[(bd % 3) as usize];
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
            bd = bd.saturating_add(1);
        } else if ch == '}' || ch == ')' || ch == ']' {
            bd = bd.saturating_sub(1);
            let color = bracket_colors[(bd % 3) as usize];
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
        } else {
            spans.push(Span::styled(ch.to_string(), base));
        }
        i += ch.len_utf8();
    }
    Line::from(spans)
}
#[cfg(test)]
mod syntax_and_lang_tests {
    use super::*;
    use crate::util::{comment_prefix_for_path, leading_indent_bytes};
    use ratatui::style::Color;
    use std::path::Path;

    const BC: [Color; 3] = [
        Color::Rgb(210, 168, 75),
        Color::Rgb(176, 82, 204),
        Color::Rgb(0, 175, 215),
    ];

    fn create_test_theme() -> Theme {
        Theme {
            name: "test_theme".to_string(),
            theme_type: "dark".to_string(),
            bg: Color::Rgb(30, 30, 30),
            bg_alt: Color::Rgb(40, 40, 40),
            fg: Color::Rgb(220, 220, 220),
            fg_muted: Color::Rgb(100, 100, 120),
            border: Color::Rgb(100, 100, 100),
            accent: Color::Rgb(86, 156, 214),
            accent_secondary: Color::Rgb(206, 198, 130),
            selection: Color::Rgb(60, 60, 60),
            comment: Color::Rgb(100, 100, 120),
            syntax_string: Color::Rgb(156, 220, 140),
            syntax_number: Color::Rgb(181, 206, 168),
            syntax_tag: Color::Rgb(86, 156, 214),
            syntax_attribute: Color::Rgb(78, 201, 176),
            bracket_1: Color::Rgb(210, 168, 75),
            bracket_2: Color::Rgb(176, 82, 204),
            bracket_3: Color::Rgb(0, 175, 215),
        }
    }

    #[test]
    fn test_syntax_lang_for_path_rust() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("test.rs"))),
            SyntaxLang::Rust
        );
    }

    #[test]
    fn test_syntax_lang_for_path_python() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("test.py"))),
            SyntaxLang::Python
        );
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("test.pyi"))),
            SyntaxLang::Python
        );
    }

    #[test]
    fn test_syntax_lang_for_path_javascript_typescript() {
        for file in &[
            "test.js", "test.jsx", "test.ts", "test.tsx", "test.mjs", "test.cjs", "test.mts",
            "test.cts",
        ] {
            assert_eq!(
                syntax_lang_for_path(Some(Path::new(file))),
                SyntaxLang::JsTs,
                "Failed for {}",
                file
            );
        }
    }

    #[test]
    fn test_syntax_lang_for_path_go() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("main.go"))),
            SyntaxLang::Go
        );
    }

    #[test]
    fn test_syntax_lang_for_path_php() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("index.php"))),
            SyntaxLang::Php
        );
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("page.phtml"))),
            SyntaxLang::Php
        );
    }

    #[test]
    fn test_syntax_lang_for_path_css() {
        for file in &["style.css", "app.scss", "design.sass", "theme.less"] {
            assert_eq!(
                syntax_lang_for_path(Some(Path::new(file))),
                SyntaxLang::Css,
                "Failed for {}",
                file
            );
        }
    }

    #[test]
    fn test_syntax_lang_for_path_html_xml() {
        for file in &[
            "index.html",
            "page.htm",
            "data.xml",
            "icon.svg",
            "doc.xhtml",
            "App.vue",
            "Component.svelte",
            "Page.astro",
            "page.jsp",
            "template.erb",
            "partial.hbs",
            "view.ejs",
        ] {
            assert_eq!(
                syntax_lang_for_path(Some(Path::new(file))),
                SyntaxLang::HtmlXml,
                "Failed for {}",
                file
            );
        }
    }

    #[test]
    fn test_syntax_lang_for_path_shell() {
        for file in &["script.sh", "init.bash", "setup.zsh", "run.fish", "env.ksh"] {
            assert_eq!(
                syntax_lang_for_path(Some(Path::new(file))),
                SyntaxLang::Shell,
                "Failed for {}",
                file
            );
        }
    }

    #[test]
    fn test_syntax_lang_for_path_json() {
        for file in &[
            "package.json",
            "config.jsonc",
            "Cargo.toml",
            "config.yaml",
            "data.yml",
        ] {
            assert_eq!(
                syntax_lang_for_path(Some(Path::new(file))),
                SyntaxLang::Json,
                "Failed for {}",
                file
            );
        }
    }

    #[test]
    fn test_syntax_lang_for_path_markdown() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("README.md"))),
            SyntaxLang::Markdown
        );
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("NOTES.markdown"))),
            SyntaxLang::Markdown
        );
    }

    #[test]
    fn test_syntax_lang_for_path_unknown_extension() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("test.xyz"))),
            SyntaxLang::Plain
        );
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("test.unknown"))),
            SyntaxLang::Plain
        );
    }

    #[test]
    fn test_syntax_lang_for_path_no_extension() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("Makefile"))),
            SyntaxLang::Plain
        );
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("README"))),
            SyntaxLang::Plain
        );
    }

    #[test]
    fn test_syntax_lang_for_path_none() {
        assert_eq!(syntax_lang_for_path(None), SyntaxLang::Plain);
    }

    #[test]
    fn test_syntax_lang_for_path_case_insensitive() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("test.RS"))),
            SyntaxLang::Rust
        );
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("test.PY"))),
            SyntaxLang::Python
        );
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("test.HTML"))),
            SyntaxLang::HtmlXml
        );
    }

    #[test]
    fn test_is_ident_char_alphanumeric() {
        assert!(is_ident_char('a'));
        assert!(is_ident_char('z'));
        assert!(is_ident_char('A'));
        assert!(is_ident_char('Z'));
        assert!(is_ident_char('0'));
        assert!(is_ident_char('9'));
    }

    #[test]
    fn test_is_ident_char_underscore() {
        assert!(is_ident_char('_'));
    }

    #[test]
    fn test_is_ident_char_special_chars() {
        assert!(!is_ident_char('-'));
        assert!(!is_ident_char('.'));
        assert!(!is_ident_char('!'));
        assert!(!is_ident_char('@'));
        assert!(!is_ident_char(' '));
        assert!(!is_ident_char('\t'));
        assert!(!is_ident_char('\n'));
    }

    #[test]
    fn test_is_ident_char_unicode() {
        assert!(!is_ident_char('ñ'));
        assert!(!is_ident_char('ü'));
        assert!(!is_ident_char('中'));
        assert!(!is_ident_char('🦀'));
    }

    #[test]
    fn test_keywords_for_lang_rust() {
        let keywords = keywords_for_lang(SyntaxLang::Rust);
        assert!(!keywords.is_empty());
        assert!(keywords.contains(&"fn"));
        assert!(keywords.contains(&"let"));
        assert!(keywords.contains(&"mut"));
        assert!(keywords.contains(&"struct"));
        assert!(keywords.contains(&"enum"));
        assert!(keywords.contains(&"impl"));
    }

    #[test]
    fn test_keywords_for_lang_python() {
        let keywords = keywords_for_lang(SyntaxLang::Python);
        assert!(!keywords.is_empty());
        assert!(keywords.contains(&"def"));
        assert!(keywords.contains(&"class"));
        assert!(keywords.contains(&"import"));
        assert!(keywords.contains(&"None"));
        assert!(keywords.contains(&"True"));
        assert!(keywords.contains(&"False"));
    }

    #[test]
    fn test_keywords_for_lang_jsts() {
        let keywords = keywords_for_lang(SyntaxLang::JsTs);
        assert!(!keywords.is_empty());
        assert!(keywords.contains(&"function"));
        assert!(keywords.contains(&"const"));
        assert!(keywords.contains(&"let"));
        assert!(keywords.contains(&"async"));
        assert!(keywords.contains(&"await"));
    }

    #[test]
    fn test_keywords_for_lang_go() {
        let keywords = keywords_for_lang(SyntaxLang::Go);
        assert!(!keywords.is_empty());
        assert!(keywords.contains(&"package"));
        assert!(keywords.contains(&"func"));
        assert!(keywords.contains(&"go"));
        assert!(keywords.contains(&"defer"));
    }

    #[test]
    fn test_keywords_for_lang_no_keywords() {
        assert!(keywords_for_lang(SyntaxLang::HtmlXml).is_empty());
        assert!(keywords_for_lang(SyntaxLang::Json).is_empty());
        assert!(keywords_for_lang(SyntaxLang::Markdown).is_empty());
        assert!(keywords_for_lang(SyntaxLang::Plain).is_empty());
    }

    #[test]
    fn test_comment_start_for_lang_slash_slash() {
        assert_eq!(comment_start_for_lang(SyntaxLang::Rust), Some("//"));
        assert_eq!(comment_start_for_lang(SyntaxLang::JsTs), Some("//"));
        assert_eq!(comment_start_for_lang(SyntaxLang::Go), Some("//"));
    }

    #[test]
    fn test_comment_start_for_lang_hash() {
        assert_eq!(comment_start_for_lang(SyntaxLang::Python), Some("#"));
        assert_eq!(comment_start_for_lang(SyntaxLang::Shell), Some("#"));
    }

    #[test]
    fn test_comment_start_for_lang_slash_star() {
        assert_eq!(comment_start_for_lang(SyntaxLang::Php), Some("/*"));
        assert_eq!(comment_start_for_lang(SyntaxLang::Css), Some("/*"));
    }

    #[test]
    fn test_comment_start_for_lang_no_comment() {
        assert_eq!(comment_start_for_lang(SyntaxLang::HtmlXml), None);
        assert_eq!(comment_start_for_lang(SyntaxLang::Json), None);
        assert_eq!(comment_start_for_lang(SyntaxLang::Markdown), None);
        assert_eq!(comment_start_for_lang(SyntaxLang::Plain), None);
    }

    #[test]
    fn test_comment_prefix_for_path_slash_slash() {
        for file in &[
            "test.rs",
            "test.js",
            "test.ts",
            "test.go",
            "test.java",
            "test.c",
            "test.cpp",
            "test.cs",
            "test.swift",
        ] {
            assert_eq!(
                comment_prefix_for_path(Path::new(file)),
                Some("//"),
                "Failed for {}",
                file
            );
        }
    }

    #[test]
    fn test_comment_prefix_for_path_hash() {
        for file in &[
            "test.py",
            "test.sh",
            "test.bash",
            "test.zsh",
            "config.yaml",
            "config.yml",
            "Cargo.toml",
            "test.rb",
        ] {
            assert_eq!(
                comment_prefix_for_path(Path::new(file)),
                Some("#"),
                "Failed for {}",
                file
            );
        }
    }

    #[test]
    fn test_comment_prefix_for_path_html_not_supported() {
        // comment_prefix_for_path doesn't handle html/xml
        assert_eq!(comment_prefix_for_path(Path::new("index.html")), None);
        assert_eq!(comment_prefix_for_path(Path::new("data.xml")), None);
    }

    #[test]
    fn test_comment_prefix_for_path_unknown() {
        assert_eq!(comment_prefix_for_path(Path::new("Makefile")), None);
        assert_eq!(comment_prefix_for_path(Path::new("test.xyz")), None);
    }

    #[test]
    fn test_leading_indent_bytes_no_indent() {
        assert_eq!(leading_indent_bytes("hello world"), 0);
        assert_eq!(leading_indent_bytes("fn main() {"), 0);
    }

    #[test]
    fn test_leading_indent_bytes_spaces() {
        assert_eq!(leading_indent_bytes("  hello"), 2);
        assert_eq!(leading_indent_bytes("    fn test() {"), 4);
        assert_eq!(leading_indent_bytes("        nested"), 8);
    }

    #[test]
    fn test_leading_indent_bytes_tabs() {
        assert_eq!(leading_indent_bytes("\thello"), 1);
        assert_eq!(leading_indent_bytes("\t\tfn test() {"), 2);
    }

    #[test]
    fn test_leading_indent_bytes_mixed() {
        assert_eq!(leading_indent_bytes("\t  hello"), 3);
        assert_eq!(leading_indent_bytes("  \t  fn"), 5);
    }

    #[test]
    fn test_leading_indent_bytes_empty_and_whitespace() {
        assert_eq!(leading_indent_bytes(""), 0);
        assert_eq!(leading_indent_bytes("    "), 4);
        assert_eq!(leading_indent_bytes("\t\t"), 2);
    }

    #[test]
    fn test_highlight_line_plain() {
        let theme = create_test_theme();
        let result = highlight_line("this is plain text", SyntaxLang::Plain, &theme, 0, &BC);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_rust_keyword() {
        let theme = create_test_theme();
        let result = highlight_line("fn main() {", SyntaxLang::Rust, &theme, 0, &BC);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_rust_comment() {
        let theme = create_test_theme();
        let result = highlight_line("// this is a comment", SyntaxLang::Rust, &theme, 0, &BC);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_rust_string() {
        let theme = create_test_theme();
        let result = highlight_line(
            r#"let s = "hello world";"#,
            SyntaxLang::Rust,
            &theme,
            0,
            &BC,
        );
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_python() {
        let theme = create_test_theme();
        assert!(
            !highlight_line("def hello():", SyntaxLang::Python, &theme, 0, &BC)
                .spans
                .is_empty()
        );
        assert!(
            !highlight_line("# comment", SyntaxLang::Python, &theme, 0, &BC)
                .spans
                .is_empty()
        );
    }

    #[test]
    fn test_highlight_line_js_go_shell_css_php() {
        let theme = create_test_theme();
        assert!(
            !highlight_line("function test() {", SyntaxLang::JsTs, &theme, 0, &BC)
                .spans
                .is_empty()
        );
        assert!(
            !highlight_line("package main", SyntaxLang::Go, &theme, 0, &BC)
                .spans
                .is_empty()
        );
        assert!(
            !highlight_line("if [ -f file ]; then", SyntaxLang::Shell, &theme, 0, &BC)
                .spans
                .is_empty()
        );
        assert!(
            !highlight_line("  display: flex;", SyntaxLang::Css, &theme, 0, &BC)
                .spans
                .is_empty()
        );
        assert!(
            !highlight_line("function test() {", SyntaxLang::Php, &theme, 0, &BC)
                .spans
                .is_empty()
        );
    }

    #[test]
    fn test_highlight_line_markdown() {
        let theme = create_test_theme();
        assert!(
            !highlight_line("# Heading 1", SyntaxLang::Markdown, &theme, 0, &BC)
                .spans
                .is_empty()
        );
        assert!(
            !highlight_line("Normal text", SyntaxLang::Markdown, &theme, 0, &BC)
                .spans
                .is_empty()
        );
    }

    #[test]
    fn test_highlight_line_html() {
        let theme = create_test_theme();
        assert!(
            !highlight_line(
                "<div class=\"container\">",
                SyntaxLang::HtmlXml,
                &theme,
                0,
                &BC
            )
            .spans
            .is_empty()
        );
        assert!(
            !highlight_line("<!-- comment -->", SyntaxLang::HtmlXml, &theme, 0, &BC)
                .spans
                .is_empty()
        );
    }

    #[test]
    fn test_syntax_lang_multiple_dots() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("my.test.file.rs"))),
            SyntaxLang::Rust
        );
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("config.test.json"))),
            SyntaxLang::Json
        );
    }

    #[test]
    fn test_syntax_lang_path_with_directories() {
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("src/main.rs"))),
            SyntaxLang::Rust
        );
        assert_eq!(
            syntax_lang_for_path(Some(Path::new("/usr/bin/script.py"))),
            SyntaxLang::Python
        );
    }

    #[test]
    fn test_keywords_uniqueness() {
        let rust_keywords = keywords_for_lang(SyntaxLang::Rust);
        let mut seen = std::collections::HashSet::new();
        for keyword in rust_keywords {
            assert!(seen.insert(keyword), "Duplicate keyword: {}", keyword);
        }
    }

    #[test]
    fn test_bracket_pair_colorization() {
        let theme = create_test_theme();
        let bc = [theme.bracket_1, theme.bracket_2, theme.bracket_3];
        // "{ ( ) }" — { at depth 0, ( at depth 1, ) at depth 1, } at depth 0
        let result = highlight_line("{ ( ) }", SyntaxLang::Rust, &theme, 0, &bc);
        let bracket_spans: Vec<_> = result
            .spans
            .iter()
            .filter(|s| matches!(s.content.as_ref(), "{" | "}" | "(" | ")"))
            .collect();
        assert_eq!(bracket_spans.len(), 4);
        // { and } should both be depth 0 → bracket_1 color
        let open_brace = bracket_spans[0].style.fg;
        let close_brace = bracket_spans[3].style.fg;
        assert_eq!(
            open_brace, close_brace,
            "matching brackets should have same color"
        );
        assert_eq!(open_brace, Some(theme.bracket_1));
        // ( and ) should both be depth 1 → bracket_2 color
        let open_paren = bracket_spans[1].style.fg;
        let close_paren = bracket_spans[2].style.fg;
        assert_eq!(
            open_paren, close_paren,
            "matching brackets should have same color"
        );
        assert_eq!(open_paren, Some(theme.bracket_2));
        // Different depths should differ
        assert_ne!(
            open_brace, open_paren,
            "different depth brackets should have different colors"
        );
    }
}

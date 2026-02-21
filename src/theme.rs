use std::fs;
use std::path::PathBuf;

use include_dir::{Dir, include_dir};
use ratatui::style::Color;
use serde::Deserialize;

const LOCAL_THEME_DIR: &str = "themes";
static EMBEDDED_THEMES: Dir = include_dir!("$CARGO_MANIFEST_DIR/themes");

#[derive(Debug, Clone)]
pub(crate) struct Theme {
    pub(crate) name: String,
    pub(crate) theme_type: String,
    pub(crate) bg: Color,
    pub(crate) bg_alt: Color,
    pub(crate) fg: Color,
    pub(crate) fg_muted: Color,
    pub(crate) border: Color,
    pub(crate) accent: Color,
    pub(crate) accent_secondary: Color,
    pub(crate) selection: Color,
    pub(crate) comment: Color,
    pub(crate) syntax_string: Color,
    pub(crate) syntax_number: Color,
    pub(crate) syntax_tag: Color,
    pub(crate) syntax_attribute: Color,
    pub(crate) bracket_1: Color,
    pub(crate) bracket_2: Color,
    pub(crate) bracket_3: Color,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ThemeFile {
    pub(crate) name: String,
    #[serde(rename = "type")]
    pub(crate) theme_type: String,
    pub(crate) colors: ThemeColors,
    #[serde(default)]
    pub(crate) syntax: Option<ThemeSyntaxColors>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ThemeColors {
    pub(crate) background: String,
    #[serde(rename = "backgroundAlt")]
    pub(crate) background_alt: String,
    pub(crate) foreground: String,
    #[serde(rename = "foregroundMuted")]
    pub(crate) foreground_muted: String,
    pub(crate) border: String,
    pub(crate) accent: String,
    #[serde(default, rename = "accentSecondary")]
    pub(crate) accent_secondary: Option<String>,
    pub(crate) selection: String,
    #[serde(default)]
    pub(crate) yellow: Option<String>,
    #[serde(default)]
    pub(crate) purple: Option<String>,
    #[serde(default)]
    pub(crate) cyan: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ThemeSyntaxColors {
    #[serde(default)]
    pub(crate) comment: Option<String>,
    #[serde(default)]
    pub(crate) string: Option<String>,
    #[serde(default)]
    pub(crate) number: Option<String>,
    #[serde(default)]
    pub(crate) tag: Option<String>,
    #[serde(default)]
    pub(crate) attribute: Option<String>,
}

pub(crate) fn color_from_hex(input: &str, fallback: Color) -> Color {
    let s = input.trim();
    if let Some(stripped) = s.strip_prefix('#')
        && stripped.len() == 6
    {
        let r = u8::from_str_radix(&stripped[0..2], 16).ok();
        let g = u8::from_str_radix(&stripped[2..4], 16).ok();
        let b = u8::from_str_radix(&stripped[4..6], 16).ok();
        if let (Some(r), Some(g), Some(b)) = (r, g, b) {
            return Color::Rgb(r, g, b);
        }
    }
    fallback
}

pub(crate) fn theme_from_file(tf: ThemeFile) -> Theme {
    let syn = tf.syntax.as_ref();
    let border_color = color_from_hex(&tf.colors.border, Color::Rgb(127, 122, 88));
    let fg_muted = color_from_hex(&tf.colors.foreground_muted, Color::Rgb(100, 100, 120));
    Theme {
        name: tf.name,
        theme_type: tf.theme_type,
        bg: color_from_hex(&tf.colors.background, Color::Rgb(20, 22, 31)),
        bg_alt: color_from_hex(&tf.colors.background_alt, Color::Rgb(25, 28, 39)),
        fg: color_from_hex(&tf.colors.foreground, Color::Rgb(215, 213, 189)),
        fg_muted,
        border: border_color,
        accent: color_from_hex(&tf.colors.accent, Color::Rgb(206, 198, 130)),
        accent_secondary: tf
            .colors
            .accent_secondary
            .as_ref()
            .map_or(Color::Rgb(86, 156, 214), |c| {
                color_from_hex(c, Color::Rgb(86, 156, 214))
            }),
        selection: color_from_hex(&tf.colors.selection, Color::Rgb(51, 70, 124)),
        comment: syn
            .and_then(|s| s.comment.as_ref())
            .map_or(fg_muted, |c| color_from_hex(c, fg_muted)),
        syntax_string: syn
            .and_then(|s| s.string.as_ref())
            .map_or(Color::Rgb(156, 220, 140), |c| {
                color_from_hex(c, Color::Rgb(156, 220, 140))
            }),
        syntax_number: syn
            .and_then(|s| s.number.as_ref())
            .map_or(Color::Rgb(181, 206, 168), |c| {
                color_from_hex(c, Color::Rgb(181, 206, 168))
            }),
        syntax_tag: syn
            .and_then(|s| s.tag.as_ref())
            .map_or(Color::Rgb(86, 156, 214), |c| {
                color_from_hex(c, Color::Rgb(86, 156, 214))
            }),
        syntax_attribute: syn
            .and_then(|s| s.attribute.as_ref())
            .map_or(Color::Rgb(78, 201, 176), |c| {
                color_from_hex(c, Color::Rgb(78, 201, 176))
            }),
        bracket_1: tf
            .colors
            .yellow
            .as_ref()
            .map_or(Color::Rgb(210, 168, 75), |c| {
                color_from_hex(c, Color::Rgb(210, 168, 75))
            }),
        bracket_2: tf
            .colors
            .purple
            .as_ref()
            .map_or(Color::Rgb(176, 82, 204), |c| {
                color_from_hex(c, Color::Rgb(176, 82, 204))
            }),
        bracket_3: tf
            .colors
            .cyan
            .as_ref()
            .map_or(Color::Rgb(0, 175, 215), |c| {
                color_from_hex(c, Color::Rgb(0, 175, 215))
            }),
    }
}

pub(crate) fn load_themes() -> Vec<Theme> {
    let mut themes = Vec::new();

    let mut theme_dirs = vec![PathBuf::from(LOCAL_THEME_DIR)];
    theme_dirs.push(PathBuf::from("/opt/homebrew/share/lazyide/themes"));
    theme_dirs.push(PathBuf::from("/usr/local/share/lazyide/themes"));

    for theme_dir in &theme_dirs {
        if !theme_dir.exists() {
            continue;
        }
        let mut paths: Vec<PathBuf> = fs::read_dir(theme_dir)
            .ok()
            .into_iter()
            .flat_map(|rd| rd.filter_map(Result::ok))
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();

        for path in paths {
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(tf) = serde_json::from_str::<ThemeFile>(&raw) else {
                continue;
            };
            themes.push(theme_from_file(tf));
        }
        if !themes.is_empty() {
            break;
        }
    }
    if themes.is_empty() {
        let mut files: Vec<_> = EMBEDDED_THEMES
            .files()
            .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort_by_key(|f| f.path());
        for file in files {
            let Some(raw) = file.contents_utf8() else {
                continue;
            };
            let Ok(tf) = serde_json::from_str::<ThemeFile>(raw) else {
                continue;
            };
            themes.push(theme_from_file(tf));
        }
    }
    themes.sort_by_key(|t| (t.theme_type != "dark", t.name.to_ascii_lowercase()));
    themes
}
#[cfg(test)]
mod theme_and_persistence_tests {
    use super::*;
    use crate::persistence::PersistedState;
    use ratatui::style::Color;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_theme_file_deserialize_all_fields() {
        let json = r##"{"name":"Test Theme","type":"dark","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","accent":"#7aa2f7","selection":"#364a82"}}"##;
        let tf: ThemeFile = serde_json::from_str(json).unwrap();
        assert_eq!(tf.name, "Test Theme");
        assert_eq!(tf.theme_type, "dark");
        assert_eq!(tf.colors.background, "#1a1b26");
        assert_eq!(tf.colors.background_alt, "#16161e");
        assert_eq!(tf.colors.foreground, "#a9b1d6");
        assert_eq!(tf.colors.border, "#414868");
        assert_eq!(tf.colors.accent, "#7aa2f7");
        assert_eq!(tf.colors.selection, "#364a82");
    }

    #[test]
    fn test_theme_file_deserialize_missing_required_field() {
        let json = r##"{"type":"dark","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","accent":"#7aa2f7","selection":"#364a82"}}"##;
        assert!(serde_json::from_str::<ThemeFile>(json).is_err());
    }

    #[test]
    fn test_theme_file_deserialize_missing_color_field() {
        let json = r##"{"name":"Incomplete","type":"dark","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","selection":"#364a82"}}"##;
        assert!(serde_json::from_str::<ThemeFile>(json).is_err());
    }

    #[test]
    fn test_theme_file_deserialize_extra_fields_ignored() {
        let json = r##"{"name":"Theme With Extras","type":"dark","source":"https://example.com","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","accent":"#7aa2f7","selection":"#364a82"},"syntax":{"keyword":"#7aa2f7"}}"##;
        let tf: ThemeFile = serde_json::from_str(json).unwrap();
        assert_eq!(tf.name, "Theme With Extras");
    }

    #[test]
    fn test_persisted_state_round_trip() {
        let state = PersistedState {
            theme_name: "Dracula".to_string(),
            files_pane_width: Some(30),
            word_wrap: Some(true),
        };
        let json = serde_json::to_string(&state).unwrap();
        let de: PersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(de.theme_name, "Dracula");
        assert_eq!(de.files_pane_width, Some(30));
        assert_eq!(de.word_wrap, Some(true));
    }

    #[test]
    fn test_persisted_state_round_trip_without_optional() {
        let state = PersistedState {
            theme_name: "Nord".to_string(),
            files_pane_width: None,
            word_wrap: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let de: PersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(de.theme_name, "Nord");
        assert_eq!(de.files_pane_width, None);
        assert_eq!(de.word_wrap, None);
    }

    #[test]
    fn test_persisted_state_missing_optional_defaults() {
        let de: PersistedState = serde_json::from_str(r##"{"theme_name":"Monokai Pro"}"##).unwrap();
        assert_eq!(de.theme_name, "Monokai Pro");
        assert_eq!(de.files_pane_width, None);
        assert_eq!(de.word_wrap, None);
    }

    #[test]
    fn test_persisted_state_missing_required_fails() {
        assert!(serde_json::from_str::<PersistedState>(r##"{"files_pane_width":20}"##).is_err());
    }

    #[test]
    fn test_theme_conversion_valid_colors() {
        let json = r##"{"name":"Conversion Test","type":"dark","colors":{"background":"#1a1b26","backgroundAlt":"#16161e","foreground":"#a9b1d6","foregroundMuted":"#565f89","border":"#414868","accent":"#7aa2f7","selection":"#364a82"},"syntax":{"comment":"#565f89","string":"#9ece6a","number":"#ff9e64","tag":"#7aa2f7","attribute":"#73daca"}}"##;
        let tf: ThemeFile = serde_json::from_str(json).unwrap();
        let theme = theme_from_file(tf);
        assert_eq!(theme.bg, Color::Rgb(26, 27, 38));
        assert_eq!(theme.fg, Color::Rgb(169, 177, 214));
        assert_eq!(theme.accent, Color::Rgb(122, 162, 247));
        assert_eq!(theme.fg_muted, Color::Rgb(86, 95, 137));
        assert_eq!(theme.comment, Color::Rgb(86, 95, 137));
        assert_eq!(theme.syntax_string, Color::Rgb(158, 206, 106));
        assert_eq!(theme.syntax_number, Color::Rgb(255, 158, 100));
        assert_eq!(theme.syntax_tag, Color::Rgb(122, 162, 247));
        assert_eq!(theme.syntax_attribute, Color::Rgb(115, 218, 202));
    }

    #[test]
    fn test_theme_conversion_invalid_colors_use_fallback() {
        let json = r##"{"name":"Fallback Test","type":"light","colors":{"background":"invalid","backgroundAlt":"not-hex","foreground":"short","foregroundMuted":"#000000","border":"","accent":"notacolor","selection":"#ffffff"}}"##;
        let tf: ThemeFile = serde_json::from_str(json).unwrap();
        let theme = theme_from_file(tf);
        assert_eq!(theme.bg, Color::Rgb(20, 22, 31));
        assert_eq!(theme.border, Color::Rgb(127, 122, 88));
        assert_eq!(theme.selection, Color::Rgb(255, 255, 255));
        // No syntax section → falls back to defaults
        assert_eq!(theme.syntax_string, Color::Rgb(156, 220, 140));
        assert_eq!(theme.syntax_number, Color::Rgb(181, 206, 168));
    }

    // Note: load_themes() tests that use set_current_dir are omitted because
    // they race with parallel test execution. Theme loading is tested indirectly
    // via the actual theme file validation tests below.

    #[test]
    fn test_all_actual_themes_deserialize() {
        let themes_dir = PathBuf::from("themes");
        if !themes_dir.exists() {
            panic!("themes/ directory not found");
        }

        let mut count = 0;
        let mut failures = Vec::new();
        for entry in fs::read_dir(&themes_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                count += 1;
                let json = fs::read_to_string(&path).unwrap();
                if let Err(e) = serde_json::from_str::<ThemeFile>(&json) {
                    failures.push(format!("{:?}: {}", path.file_name().unwrap(), e));
                }
            }
        }
        assert!(count > 0, "Should find at least one theme file");
        if !failures.is_empty() {
            panic!("Failed to deserialize themes:\n{}", failures.join("\n"));
        }
    }

    #[test]
    fn test_all_actual_themes_have_valid_hex_colors() {
        let themes_dir = PathBuf::from("themes");
        if !themes_dir.exists() {
            return;
        }

        fn is_valid_hex(s: &str) -> bool {
            let s = s.trim();
            if let Some(hex) = s.strip_prefix('#') {
                // Accept 6-char (#RRGGBB) or 8-char (#RRGGBBAA) hex
                (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
            } else {
                false
            }
        }

        let mut failures = Vec::new();
        for entry in fs::read_dir(&themes_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let json = fs::read_to_string(&path).unwrap();
                let tf: ThemeFile = match serde_json::from_str(&json) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                for (field, val) in [
                    ("background", &tf.colors.background),
                    ("backgroundAlt", &tf.colors.background_alt),
                    ("foreground", &tf.colors.foreground),
                    ("border", &tf.colors.border),
                    ("accent", &tf.colors.accent),
                    ("selection", &tf.colors.selection),
                ] {
                    if !is_valid_hex(val) {
                        failures.push(format!("{}: Invalid '{}' in '{}'", tf.name, val, field));
                    }
                }
            }
        }
        if !failures.is_empty() {
            panic!("Invalid hex colors:\n{}", failures.join("\n"));
        }
    }

    #[test]
    fn test_all_actual_themes_have_valid_type() {
        let themes_dir = PathBuf::from("themes");
        if !themes_dir.exists() {
            return;
        }

        for entry in fs::read_dir(&themes_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let json = fs::read_to_string(&path).unwrap();
                let tf: ThemeFile = match serde_json::from_str(&json) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                assert!(
                    !tf.name.is_empty(),
                    "{:?}: Empty name",
                    path.file_name().unwrap()
                );
                assert!(
                    tf.theme_type == "dark" || tf.theme_type == "light",
                    "{}: Invalid type '{}'",
                    tf.name,
                    tf.theme_type
                );
            }
        }
    }
}

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

const KEYBINDS_FILE_REL: &str = "lazyide/keybinds.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeyAction {
    // Global
    Save,
    CloseTab,
    Quit,
    ToggleFiles,
    CommandPalette,
    QuickOpen,
    Find,
    FindReplace,
    SearchFiles,
    GoToLine,
    Help,
    NewFile,
    RefreshTree,
    PrevTab,
    NextTab,
    ToggleWordWrap,
    // Editor
    GoToDefinition,
    FoldToggle,
    FoldAllToggle,
    Fold,
    Unfold,
    FoldAll,
    UnfoldAll,
    FindNext,
    FindPrev,
    DupLineDown,
    DupLineUp,
    Dedent,
    Completion,
    Undo,
    Redo,
    SelectAll,
    Copy,
    Cut,
    CutLine,
    Paste,
    ToggleComment,
    PageDown,
    PageUp,
    GoToStart,
    GoToEnd,
}

impl KeyAction {
    pub(crate) fn is_global(self) -> bool {
        matches!(
            self,
            KeyAction::Save
                | KeyAction::CloseTab
                | KeyAction::Quit
                | KeyAction::ToggleFiles
                | KeyAction::CommandPalette
                | KeyAction::QuickOpen
                | KeyAction::Find
                | KeyAction::FindReplace
                | KeyAction::SearchFiles
                | KeyAction::GoToLine
                | KeyAction::Help
                | KeyAction::NewFile
                | KeyAction::RefreshTree
                | KeyAction::PrevTab
                | KeyAction::NextTab
                | KeyAction::ToggleWordWrap
        )
    }

    pub(crate) fn is_editor(self) -> bool {
        !self.is_global()
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            KeyAction::Save => "Save",
            KeyAction::CloseTab => "Close Tab",
            KeyAction::Quit => "Quit",
            KeyAction::ToggleFiles => "Toggle Files",
            KeyAction::CommandPalette => "Command Palette",
            KeyAction::QuickOpen => "Quick Open",
            KeyAction::Find => "Find",
            KeyAction::FindReplace => "Find & Replace",
            KeyAction::SearchFiles => "Search Files",
            KeyAction::GoToLine => "Go to Line",
            KeyAction::Help => "Help",
            KeyAction::NewFile => "New File",
            KeyAction::RefreshTree => "Refresh Tree",
            KeyAction::PrevTab => "Previous Tab",
            KeyAction::NextTab => "Next Tab",
            KeyAction::ToggleWordWrap => "Toggle Word Wrap",
            KeyAction::GoToDefinition => "Go to Definition",
            KeyAction::FoldToggle => "Toggle Fold",
            KeyAction::FoldAllToggle => "Toggle Fold All",
            KeyAction::Fold => "Fold",
            KeyAction::Unfold => "Unfold",
            KeyAction::FoldAll => "Fold All",
            KeyAction::UnfoldAll => "Unfold All",
            KeyAction::FindNext => "Find Next",
            KeyAction::FindPrev => "Find Previous",
            KeyAction::DupLineDown => "Duplicate Line Down",
            KeyAction::DupLineUp => "Duplicate Line Up",
            KeyAction::Dedent => "Dedent",
            KeyAction::Completion => "Completion",
            KeyAction::Undo => "Undo",
            KeyAction::Redo => "Redo",
            KeyAction::SelectAll => "Select All",
            KeyAction::Copy => "Copy",
            KeyAction::Cut => "Cut",
            KeyAction::CutLine => "Cut Line",
            KeyAction::Paste => "Paste",
            KeyAction::ToggleComment => "Toggle Comment",
            KeyAction::PageDown => "Page Down",
            KeyAction::PageUp => "Page Up",
            KeyAction::GoToStart => "Go to Start",
            KeyAction::GoToEnd => "Go to End",
        }
    }

    pub(crate) fn all() -> &'static [KeyAction] {
        &[
            KeyAction::Save,
            KeyAction::CloseTab,
            KeyAction::Quit,
            KeyAction::ToggleFiles,
            KeyAction::CommandPalette,
            KeyAction::QuickOpen,
            KeyAction::Find,
            KeyAction::FindReplace,
            KeyAction::SearchFiles,
            KeyAction::GoToLine,
            KeyAction::Help,
            KeyAction::NewFile,
            KeyAction::RefreshTree,
            KeyAction::PrevTab,
            KeyAction::NextTab,
            KeyAction::ToggleWordWrap,
            KeyAction::GoToDefinition,
            KeyAction::FoldToggle,
            KeyAction::FoldAllToggle,
            KeyAction::Fold,
            KeyAction::Unfold,
            KeyAction::FoldAll,
            KeyAction::UnfoldAll,
            KeyAction::FindNext,
            KeyAction::FindPrev,
            KeyAction::DupLineDown,
            KeyAction::DupLineUp,
            KeyAction::Dedent,
            KeyAction::Completion,
            KeyAction::Undo,
            KeyAction::Redo,
            KeyAction::SelectAll,
            KeyAction::Copy,
            KeyAction::Cut,
            KeyAction::CutLine,
            KeyAction::Paste,
            KeyAction::ToggleComment,
            KeyAction::PageDown,
            KeyAction::PageUp,
            KeyAction::GoToStart,
            KeyAction::GoToEnd,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyBind {
    pub(crate) modifiers: KeyModifiers,
    pub(crate) code: KeyCode,
}

impl KeyBind {
    pub(crate) fn normalize_char_with_modifiers(code: KeyCode, modifiers: KeyModifiers) -> KeyCode {
        match code {
            KeyCode::Char(c) if modifiers.contains(KeyModifiers::CONTROL) => {
                let u = c as u32;
                if (1..=26).contains(&u) {
                    let letter = (b'a' + (u as u8) - 1) as char;
                    KeyCode::Char(letter)
                } else {
                    KeyCode::Char(c)
                }
            }
            other => other,
        }
    }

    pub(crate) fn parse(s: &str) -> Option<KeyBind> {
        let parts: Vec<&str> = s.split('+').collect();
        if parts.is_empty() {
            return None;
        }
        let mut modifiers = KeyModifiers::NONE;
        for &part in &parts[..parts.len() - 1] {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" => modifiers |= KeyModifiers::CONTROL,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                "alt" => modifiers |= KeyModifiers::ALT,
                _ => return None,
            }
        }
        let key_str = parts.last()?;
        let lower = key_str.to_ascii_lowercase();
        let code = match lower.as_str() {
            " " | "space" => KeyCode::Char(' '),
            "esc" | "escape" => KeyCode::Esc,
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "f1" => KeyCode::F(1),
            "f2" => KeyCode::F(2),
            "f3" => KeyCode::F(3),
            "f4" => KeyCode::F(4),
            "f5" => KeyCode::F(5),
            "f6" => KeyCode::F(6),
            "f7" => KeyCode::F(7),
            "f8" => KeyCode::F(8),
            "f9" => KeyCode::F(9),
            "f10" => KeyCode::F(10),
            "f11" => KeyCode::F(11),
            "f12" => KeyCode::F(12),
            _ => {
                let mut chars = lower.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => KeyCode::Char(c),
                    _ => return None,
                }
            }
        };
        Some(KeyBind { modifiers, code })
    }

    fn format_key_name(&self, uppercase: bool) -> String {
        match self.code {
            KeyCode::Char(c) => {
                if c == ' ' {
                    if uppercase {
                        "Space".to_string()
                    } else {
                        "space".to_string()
                    }
                } else if uppercase {
                    c.to_ascii_uppercase().to_string()
                } else {
                    c.to_ascii_lowercase().to_string()
                }
            }
            KeyCode::F(n) => {
                if uppercase {
                    format!("F{n}")
                } else {
                    format!("f{n}")
                }
            }
            KeyCode::Esc => {
                if uppercase {
                    "Esc".to_string()
                } else {
                    "esc".to_string()
                }
            }
            KeyCode::Enter => {
                if uppercase {
                    "Enter".to_string()
                } else {
                    "enter".to_string()
                }
            }
            KeyCode::Tab => {
                if uppercase {
                    "Tab".to_string()
                } else {
                    "tab".to_string()
                }
            }
            KeyCode::BackTab => {
                if uppercase {
                    "BackTab".to_string()
                } else {
                    "backtab".to_string()
                }
            }
            KeyCode::Backspace => {
                if uppercase {
                    "Backspace".to_string()
                } else {
                    "backspace".to_string()
                }
            }
            KeyCode::Delete => {
                if uppercase {
                    "Delete".to_string()
                } else {
                    "delete".to_string()
                }
            }
            KeyCode::Up => {
                if uppercase {
                    "Up".to_string()
                } else {
                    "up".to_string()
                }
            }
            KeyCode::Down => {
                if uppercase {
                    "Down".to_string()
                } else {
                    "down".to_string()
                }
            }
            KeyCode::Left => {
                if uppercase {
                    "Left".to_string()
                } else {
                    "left".to_string()
                }
            }
            KeyCode::Right => {
                if uppercase {
                    "Right".to_string()
                } else {
                    "right".to_string()
                }
            }
            KeyCode::Home => {
                if uppercase {
                    "Home".to_string()
                } else {
                    "home".to_string()
                }
            }
            KeyCode::End => {
                if uppercase {
                    "End".to_string()
                } else {
                    "end".to_string()
                }
            }
            KeyCode::PageUp => {
                if uppercase {
                    "PageUp".to_string()
                } else {
                    "pageup".to_string()
                }
            }
            KeyCode::PageDown => {
                if uppercase {
                    "PageDown".to_string()
                } else {
                    "pagedown".to_string()
                }
            }
            _ => "?".to_string(),
        }
    }

    fn format_bind(&self, uppercase: bool) -> String {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push(if uppercase { "Ctrl" } else { "ctrl" }.to_string());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push(if uppercase { "Shift" } else { "shift" }.to_string());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push(if uppercase { "Alt" } else { "alt" }.to_string());
        }
        parts.push(self.format_key_name(uppercase));
        parts.join("+")
    }

    pub(crate) fn display(&self) -> String {
        if cfg!(target_os = "macos") {
            self.format_bind(true)
                .replace("Alt+", "⌥+")
                .replace("Ctrl+", "⌃+")
                .replace("Shift+", "⇧+")
        } else {
            self.format_bind(true)
        }
    }

    pub(crate) fn matches(&self, key: &KeyEvent) -> bool {
        // For letter keys, crossterm may set SHIFT bit for uppercase.
        // Normalize: strip SHIFT from both sides when comparing Char keys.
        let (bind_mods, bind_code) = (
            self.modifiers,
            KeyBind::normalize_char_with_modifiers(self.code, self.modifiers),
        );
        let (mut ev_mods, ev_code) = (
            key.modifiers,
            KeyBind::normalize_char_with_modifiers(key.code, key.modifiers),
        );
        // Normalize the event char to lowercase for comparison
        let ev_code_normalized = match ev_code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        let bind_code_normalized = match bind_code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        // For Char keys, ignore SHIFT flag since we normalize case
        if matches!(ev_code, KeyCode::Char(_)) {
            ev_mods -= KeyModifiers::SHIFT;
        }
        let mut bind_mods_cmp = bind_mods;
        if matches!(bind_code, KeyCode::Char(_)) {
            bind_mods_cmp -= KeyModifiers::SHIFT;
        }
        // Handle Shift+BackTab special case: crossterm sends BackTab when Shift+Tab
        if bind_code == KeyCode::BackTab && ev_code == KeyCode::BackTab {
            // BackTab inherently means Shift+Tab, so ignore SHIFT in mods
            let ev_no_shift = ev_mods - KeyModifiers::SHIFT;
            let bind_no_shift = bind_mods - KeyModifiers::SHIFT;
            return ev_no_shift == bind_no_shift;
        }
        // For bracket chars with Shift (e.g. Ctrl+Shift+[), crossterm may report
        // the shifted char ('{') instead of '['. Check if binding uses Shift+char
        // and compare against the shifted variant.
        if bind_mods.contains(KeyModifiers::SHIFT) && matches!(bind_code, KeyCode::Char(_)) {
            let shifted = match bind_code {
                KeyCode::Char('[') => Some(KeyCode::Char('{')),
                KeyCode::Char(']') => Some(KeyCode::Char('}')),
                _ => None,
            };
            if let Some(shifted_code) = shifted {
                let shifted_normalized = match shifted_code {
                    KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
                    other => other,
                };
                if ev_code_normalized == shifted_normalized {
                    let ev_no_shift = ev_mods - KeyModifiers::SHIFT;
                    let bind_no_shift = bind_mods - KeyModifiers::SHIFT;
                    return ev_no_shift == bind_no_shift;
                }
            }
        }
        ev_code_normalized == bind_code_normalized && ev_mods == bind_mods_cmp
    }

    pub(crate) fn conflicts_with(&self, other: &KeyBind) -> bool {
        self.matches(&KeyEvent::new(other.code, other.modifiers))
            || other.matches(&KeyEvent::new(self.code, self.modifiers))
    }

    pub(crate) fn to_string_config(&self) -> String {
        self.format_bind(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyScope {
    Global,
    Editor,
}

#[derive(Debug, Clone)]
pub(crate) struct KeyBindings {
    pub(crate) map: HashMap<KeyAction, Vec<KeyBind>>,
}

impl KeyBindings {
    pub(crate) fn defaults() -> Self {
        let mut map: HashMap<KeyAction, Vec<KeyBind>> = HashMap::new();
        let mut bind = |action: KeyAction, s: &str| {
            map.entry(action)
                .or_default()
                .push(KeyBind::parse(s).expect("invalid default keybind"));
        };

        // Global
        bind(KeyAction::Save, "ctrl+s");
        bind(KeyAction::CloseTab, "ctrl+w");
        bind(KeyAction::Quit, "ctrl+q");
        bind(KeyAction::ToggleFiles, "ctrl+b");
        bind(KeyAction::CommandPalette, "ctrl+p");
        bind(KeyAction::CommandPalette, "ctrl+shift+p");
        bind(KeyAction::QuickOpen, "ctrl+o");
        bind(KeyAction::Find, "ctrl+f");
        bind(KeyAction::FindReplace, "ctrl+h");
        bind(KeyAction::SearchFiles, "ctrl+shift+f");
        bind(KeyAction::Help, "f4");
        bind(KeyAction::NewFile, "ctrl+n");
        bind(KeyAction::RefreshTree, "ctrl+r");
        bind(KeyAction::PrevTab, "f1");
        bind(KeyAction::NextTab, "f2");
        bind(KeyAction::ToggleWordWrap, "alt+z");
        bind(KeyAction::ToggleWordWrap, "f6");

        // Editor
        bind(KeyAction::GoToDefinition, "ctrl+d");
        bind(KeyAction::GoToDefinition, "ctrl+alt+d");
        bind(KeyAction::FoldToggle, "ctrl+j");
        bind(KeyAction::FoldAllToggle, "ctrl+u");
        bind(KeyAction::Fold, "ctrl+shift+[");
        bind(KeyAction::Unfold, "ctrl+shift+]");
        bind(KeyAction::FoldAll, "ctrl+alt+[");
        bind(KeyAction::UnfoldAll, "ctrl+alt+]");
        bind(KeyAction::FindNext, "f3");
        bind(KeyAction::FindPrev, "shift+f3");
        bind(KeyAction::DupLineDown, "shift+alt+down");
        bind(KeyAction::DupLineUp, "shift+alt+up");
        bind(KeyAction::Dedent, "shift+backtab");
        bind(KeyAction::Completion, "ctrl+space");
        bind(KeyAction::Completion, "ctrl+.");
        bind(KeyAction::GoToLine, "ctrl+g");
        bind(KeyAction::ToggleComment, "ctrl+/");
        bind(KeyAction::Undo, "ctrl+z");
        bind(KeyAction::Redo, "ctrl+shift+z");
        bind(KeyAction::Redo, "ctrl+y");
        bind(KeyAction::SelectAll, "ctrl+a");
        bind(KeyAction::Copy, "ctrl+c");
        bind(KeyAction::Cut, "ctrl+x");
        bind(KeyAction::CutLine, "ctrl+k");
        bind(KeyAction::Paste, "ctrl+v");
        bind(KeyAction::PageDown, "pagedown");
        bind(KeyAction::PageUp, "pageup");
        bind(KeyAction::GoToStart, "ctrl+home");
        bind(KeyAction::GoToEnd, "ctrl+end");

        KeyBindings { map }
    }

    pub(crate) fn lookup(&self, key: &KeyEvent, scope: KeyScope) -> Option<KeyAction> {
        for action in KeyAction::all().iter().copied() {
            let in_scope = match scope {
                KeyScope::Global => action.is_global(),
                KeyScope::Editor => action.is_editor(),
            };
            if !in_scope {
                continue;
            }
            if let Some(binds) = self.map.get(&action) {
                for bind in binds {
                    if bind.matches(key) {
                        return Some(action);
                    }
                }
            }
        }
        None
    }

    pub(crate) fn display_for(&self, action: KeyAction) -> String {
        self.map
            .get(&action)
            .and_then(|v| v.first())
            .map(|b| b.display())
            .unwrap_or_else(|| "unbound".to_string())
    }

    #[cfg(test)]
    pub(crate) fn set(&mut self, action: KeyAction, binds: Vec<KeyBind>) {
        self.map.insert(action, binds);
    }

    #[cfg(test)]
    pub(crate) fn conflicts(&self) -> Vec<(KeyBind, KeyAction, KeyAction)> {
        let mut result = Vec::new();
        let actions: Vec<_> = KeyAction::all().to_vec();
        for i in 0..actions.len() {
            for j in (i + 1)..actions.len() {
                let a1 = actions[i];
                let a2 = actions[j];
                // Only check conflicts within the same scope
                if a1.is_global() != a2.is_global() {
                    continue;
                }
                if let (Some(binds1), Some(binds2)) = (self.map.get(&a1), self.map.get(&a2)) {
                    for b1 in binds1 {
                        for b2 in binds2 {
                            if b1.conflicts_with(b2) {
                                result.push((b1.clone(), a1, a2));
                            }
                        }
                    }
                }
            }
        }
        result
    }

    pub(crate) fn find_conflict(
        &self,
        bind: &KeyBind,
        exclude_action: KeyAction,
    ) -> Option<KeyAction> {
        for action in KeyAction::all().iter().copied() {
            if action == exclude_action {
                continue;
            }
            // Warn across both scopes to match runtime dispatch:
            // global lookup runs first and can shadow editor bindings.
            if let Some(binds) = self.map.get(&action) {
                for b in binds {
                    if b.conflicts_with(bind) {
                        return Some(action);
                    }
                }
            }
        }
        None
    }

    pub(crate) fn remove_bind_from(&mut self, action: KeyAction, bind: &KeyBind) {
        if let Some(binds) = self.map.get_mut(&action) {
            binds.retain(|b| !b.conflicts_with(bind));
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum SingleOrVec {
    Single(String),
    Multiple(Vec<String>),
}

fn keybinds_file_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join(KEYBINDS_FILE_REL));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return Some(PathBuf::from(appdata).join(KEYBINDS_FILE_REL));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config").join(KEYBINDS_FILE_REL))
}

pub(crate) fn parse_key_action_name(name: &str) -> Option<KeyAction> {
    serde_json::from_value::<KeyAction>(serde_json::Value::String(name.to_string())).ok()
}

pub(crate) fn apply_keybinding_overrides(
    kb: &mut KeyBindings,
    overrides: HashMap<String, SingleOrVec>,
    source: &str,
) {
    for (action_name, val) in overrides {
        let Some(action) = parse_key_action_name(&action_name) else {
            eprintln!("lazyide: unknown key action '{action_name}' in {source}");
            continue;
        };
        let strings = match val {
            SingleOrVec::Single(s) => vec![s],
            SingleOrVec::Multiple(v) => v,
        };
        if strings.is_empty() {
            // Explicitly unbound action (e.g. "save": [])
            kb.map.insert(action, Vec::new());
            continue;
        }
        let mut binds = Vec::new();
        let mut invalid = Vec::new();
        for s in strings {
            if let Some(parsed) = KeyBind::parse(&s) {
                binds.push(parsed);
            } else {
                invalid.push(s);
            }
        }
        if !invalid.is_empty() {
            eprintln!(
                "lazyide: invalid keybind(s) for '{action_name}' in {source}: {}",
                invalid.join(", ")
            );
        }
        if !binds.is_empty() {
            kb.map.insert(action, binds);
        }
    }
}

pub(crate) fn parse_override_entry(
    action_name: &str,
    raw: serde_json::Value,
    source: &str,
) -> Option<(String, SingleOrVec)> {
    match raw {
        serde_json::Value::String(s) => Some((action_name.to_string(), SingleOrVec::Single(s))),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if let serde_json::Value::String(s) = item {
                    out.push(s);
                } else {
                    eprintln!(
                        "lazyide: invalid keybind list item for '{action_name}' in {source}: expected string"
                    );
                    return None;
                }
            }
            Some((action_name.to_string(), SingleOrVec::Multiple(out)))
        }
        _ => {
            eprintln!(
                "lazyide: invalid keybind value type for '{action_name}' in {source}: expected string or array of strings"
            );
            None
        }
    }
}

pub(crate) fn selected_action(actions: &[KeyAction], index: usize) -> Option<KeyAction> {
    actions.get(index).copied()
}

pub(crate) fn load_keybindings() -> KeyBindings {
    let mut kb = KeyBindings::defaults();
    let Some(path) = keybinds_file_path() else {
        return kb;
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return kb;
    };
    let source = path.display().to_string();
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&raw) else {
        eprintln!("lazyide: invalid keybinds json in {source}");
        return kb;
    };
    let Some(obj) = root.as_object() else {
        eprintln!("lazyide: invalid keybinds json in {source}: expected object");
        return kb;
    };
    let mut overrides: HashMap<String, SingleOrVec> = HashMap::new();
    for (action_name, raw_val) in obj {
        if let Some((k, v)) = parse_override_entry(action_name, raw_val.clone(), &source) {
            overrides.insert(k, v);
        }
    }
    apply_keybinding_overrides(&mut kb, overrides, &source);
    kb
}

pub(crate) fn save_keybindings(current: &KeyBindings) -> io::Result<()> {
    let Some(path) = keybinds_file_path() else {
        return Ok(());
    };
    let defaults = KeyBindings::defaults();
    let mut overrides: HashMap<String, serde_json::Value> = HashMap::new();
    for action in KeyAction::all() {
        let current_binds = current.map.get(action).cloned().unwrap_or_default();
        let default_binds = defaults.map.get(action).cloned().unwrap_or_default();
        if current_binds != default_binds {
            let action_name = serde_json::to_value(action).unwrap_or(serde_json::Value::Null);
            let action_str = action_name.as_str().unwrap_or("unknown").to_string();
            let bind_strs: Vec<String> =
                current_binds.iter().map(|b| b.to_string_config()).collect();
            let val = if bind_strs.len() == 1 {
                serde_json::Value::String(bind_strs.into_iter().next().unwrap())
            } else {
                serde_json::Value::Array(
                    bind_strs
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                )
            };
            overrides.insert(action_str, val);
        }
    }
    if overrides.is_empty() {
        // No overrides; remove file if it exists
        let _ = fs::remove_file(&path);
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(&overrides)
        .map_err(|e| io::Error::other(format!("serialize keybinds: {e}")))?;
    fs::write(path, raw)
}
#[cfg(test)]
mod keybind_tests {
    use super::*;

    #[test]
    fn test_keybind_parse_simple() {
        let kb = KeyBind::parse("ctrl+s").unwrap();
        assert_eq!(kb.modifiers, KeyModifiers::CONTROL);
        assert_eq!(kb.code, KeyCode::Char('s'));
    }

    #[test]
    fn test_keybind_parse_shift_modifier() {
        let kb = KeyBind::parse("ctrl+shift+f").unwrap();
        assert_eq!(kb.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(kb.code, KeyCode::Char('f'));
    }

    #[test]
    fn test_keybind_parse_alt_modifier() {
        let kb = KeyBind::parse("ctrl+alt+d").unwrap();
        assert_eq!(kb.modifiers, KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert_eq!(kb.code, KeyCode::Char('d'));
    }

    #[test]
    fn test_keybind_parse_function_key() {
        let kb = KeyBind::parse("f4").unwrap();
        assert_eq!(kb.modifiers, KeyModifiers::NONE);
        assert_eq!(kb.code, KeyCode::F(4));
    }

    #[test]
    fn test_keybind_parse_shift_f3() {
        let kb = KeyBind::parse("shift+f3").unwrap();
        assert_eq!(kb.modifiers, KeyModifiers::SHIFT);
        assert_eq!(kb.code, KeyCode::F(3));
    }

    #[test]
    fn test_keybind_parse_pagedown() {
        let kb = KeyBind::parse("pagedown").unwrap();
        assert_eq!(kb.modifiers, KeyModifiers::NONE);
        assert_eq!(kb.code, KeyCode::PageDown);
    }

    #[test]
    fn test_keybind_parse_backtab() {
        let kb = KeyBind::parse("shift+backtab").unwrap();
        assert_eq!(kb.modifiers, KeyModifiers::SHIFT);
        assert_eq!(kb.code, KeyCode::BackTab);
    }

    #[test]
    fn test_keybind_parse_space() {
        let kb = KeyBind::parse("ctrl+space").unwrap();
        assert_eq!(kb.modifiers, KeyModifiers::CONTROL);
        assert_eq!(kb.code, KeyCode::Char(' '));
    }

    #[test]
    fn test_keybind_parse_invalid() {
        assert!(KeyBind::parse("").is_none());
        assert!(KeyBind::parse("ctrl+unknown_key").is_none());
    }

    #[test]
    fn test_keybind_display() {
        let kb = KeyBind::parse("ctrl+shift+f").unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(kb.display(), "⌃+⇧+F");
        } else {
            assert_eq!(kb.display(), "Ctrl+Shift+F");
        }
    }

    #[test]
    fn test_keybind_display_simple() {
        let kb = KeyBind::parse("ctrl+s").unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(kb.display(), "⌃+S");
        } else {
            assert_eq!(kb.display(), "Ctrl+S");
        }
    }

    #[test]
    fn test_keybind_display_function_key() {
        let kb = KeyBind::parse("f4").unwrap();
        assert_eq!(kb.display(), "F4");
    }

    #[test]
    fn test_keybind_to_string_config() {
        let kb = KeyBind::parse("ctrl+shift+f").unwrap();
        assert_eq!(kb.to_string_config(), "ctrl+shift+f");
    }

    #[test]
    fn test_keybind_matches_simple() {
        let kb = KeyBind::parse("ctrl+s").unwrap();
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert!(kb.matches(&event));
    }

    #[test]
    fn test_keybind_matches_uppercase() {
        let kb = KeyBind::parse("ctrl+s").unwrap();
        let event = KeyEvent::new(
            KeyCode::Char('S'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(kb.matches(&event));
    }

    #[test]
    fn test_keybind_matches_ctrl_control_char_event() {
        let kb = KeyBind::parse("ctrl+b").unwrap();
        // Some terminals report Ctrl+B as ASCII control char 0x02.
        let event = KeyEvent::new(KeyCode::Char('\u{2}'), KeyModifiers::CONTROL);
        assert!(kb.matches(&event));
    }

    #[test]
    fn test_keybind_no_match() {
        let kb = KeyBind::parse("ctrl+s").unwrap();
        let event = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert!(!kb.matches(&event));
    }

    #[test]
    fn test_defaults_has_all_actions() {
        let kb = KeyBindings::defaults();
        for action in KeyAction::all() {
            assert!(
                kb.map.contains_key(action),
                "Default keybindings missing action: {:?}",
                action
            );
        }
    }

    #[test]
    fn test_lookup_global() {
        let kb = KeyBindings::defaults();
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(kb.lookup(&event, KeyScope::Global), Some(KeyAction::Save));
    }

    #[test]
    fn test_lookup_editor() {
        let kb = KeyBindings::defaults();
        let event = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(
            kb.lookup(&event, KeyScope::Editor),
            Some(KeyAction::FoldToggle)
        );
    }

    #[test]
    fn test_lookup_wrong_scope() {
        let kb = KeyBindings::defaults();
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        // Save is global, not editor
        assert_eq!(kb.lookup(&event, KeyScope::Editor), None);
    }

    #[test]
    fn test_display_for_action() {
        let kb = KeyBindings::defaults();
        if cfg!(target_os = "macos") {
            assert_eq!(kb.display_for(KeyAction::Save), "⌃+S");
        } else {
            assert_eq!(kb.display_for(KeyAction::Save), "Ctrl+S");
        }
        assert_eq!(kb.display_for(KeyAction::Help), "F4");
    }

    #[test]
    fn test_find_conflict() {
        let kb = KeyBindings::defaults();
        let bind = KeyBind::parse("ctrl+s").unwrap();
        // ctrl+s is bound to Save, so looking for conflicts from another action should find it
        assert_eq!(
            kb.find_conflict(&bind, KeyAction::NewFile),
            Some(KeyAction::Save)
        );
        // No conflict with itself
        assert_eq!(kb.find_conflict(&bind, KeyAction::Save), None);
    }

    #[test]
    fn test_find_conflict_matches_runtime_semantics_for_shifted_chars() {
        let kb = KeyBindings::defaults();
        let bind = KeyBind::parse("ctrl+shift+s").unwrap();
        // Runtime matching treats Ctrl+S and Ctrl+Shift+S as conflicting for Char keys.
        assert_eq!(
            kb.find_conflict(&bind, KeyAction::NewFile),
            Some(KeyAction::Save)
        );
    }

    #[test]
    fn test_find_conflict_with_control_char_bind() {
        let kb = KeyBindings::defaults();
        let bind = KeyBind {
            modifiers: KeyModifiers::CONTROL,
            code: KeyCode::Char('\u{2}'),
        };
        assert_eq!(
            kb.find_conflict(&bind, KeyAction::Quit),
            Some(KeyAction::ToggleFiles)
        );
    }

    #[test]
    fn test_remove_bind_from() {
        let mut kb = KeyBindings::defaults();
        let bind = KeyBind::parse("ctrl+s").unwrap();
        kb.remove_bind_from(KeyAction::Save, &bind);
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(kb.lookup(&event, KeyScope::Global), None);
    }

    #[test]
    fn test_key_action_is_global_editor() {
        assert!(KeyAction::Save.is_global());
        assert!(!KeyAction::Save.is_editor());
        assert!(KeyAction::Undo.is_editor());
        assert!(!KeyAction::Undo.is_global());
    }

    #[test]
    fn test_key_action_label() {
        assert_eq!(KeyAction::Save.label(), "Save");
        assert_eq!(KeyAction::GoToDefinition.label(), "Go to Definition");
        assert_eq!(KeyAction::ToggleComment.label(), "Toggle Comment");
    }

    #[test]
    fn test_key_action_serde_round_trip() {
        let action = KeyAction::Save;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"save\"");
        let parsed: KeyAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, KeyAction::Save);
    }

    #[test]
    fn test_key_action_serde_snake_case() {
        let action = KeyAction::GoToDefinition;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"go_to_definition\"");
        let parsed: KeyAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, KeyAction::GoToDefinition);
    }

    #[test]
    fn test_single_or_vec_deserialize_single() {
        let json = r#""ctrl+s""#;
        let v: SingleOrVec = serde_json::from_str(json).unwrap();
        match v {
            SingleOrVec::Single(s) => assert_eq!(s, "ctrl+s"),
            _ => panic!("Expected Single"),
        }
    }

    #[test]
    fn test_single_or_vec_deserialize_multiple() {
        let json = r#"["ctrl+shift+z", "ctrl+y"]"#;
        let v: SingleOrVec = serde_json::from_str(json).unwrap();
        match v {
            SingleOrVec::Multiple(v) => {
                assert_eq!(v.len(), 2);
                assert_eq!(v[0], "ctrl+shift+z");
                assert_eq!(v[1], "ctrl+y");
            }
            _ => panic!("Expected Multiple"),
        }
    }

    #[test]
    fn test_keybinds_json_deserialize() {
        let json = r#"{"save": "ctrl+shift+s", "redo": ["ctrl+shift+z", "ctrl+y"]}"#;
        let root: serde_json::Value = serde_json::from_str(json).unwrap();
        let obj = root.as_object().unwrap();
        let mut parsed: HashMap<String, SingleOrVec> = HashMap::new();
        for (k, v) in obj {
            if let Some((name, parsed_val)) = parse_override_entry(k, v.clone(), "test") {
                parsed.insert(name, parsed_val);
            }
        }
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains_key("save"));
        assert!(parsed.contains_key("redo"));
    }

    #[test]
    fn test_conflicts_detection() {
        let mut kb = KeyBindings::defaults();
        // Add a conflict: bind Ctrl+S to NewFile as well
        kb.map
            .entry(KeyAction::NewFile)
            .or_default()
            .push(KeyBind::parse("ctrl+s").unwrap());
        let conflicts = kb.conflicts();
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn test_set_binding() {
        let mut kb = KeyBindings::defaults();
        kb.set(KeyAction::Save, vec![KeyBind::parse("alt+s").unwrap()]);
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(kb.lookup(&event, KeyScope::Global), None);
        let event2 = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        assert_eq!(kb.lookup(&event2, KeyScope::Global), Some(KeyAction::Save));
    }

    #[test]
    fn test_lookup_deterministic_for_conflicting_actions() {
        let mut kb = KeyBindings::defaults();
        kb.map
            .insert(KeyAction::NewFile, vec![KeyBind::parse("ctrl+s").unwrap()]);
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        // Save appears before NewFile in KeyAction::all().
        assert_eq!(kb.lookup(&event, KeyScope::Global), Some(KeyAction::Save));
    }

    #[test]
    fn test_apply_overrides_explicit_empty_unbinds_action() {
        let mut kb = KeyBindings::defaults();
        let mut overrides = HashMap::new();
        overrides.insert("save".to_string(), SingleOrVec::Multiple(Vec::new()));
        apply_keybinding_overrides(&mut kb, overrides, "test");
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(kb.lookup(&event, KeyScope::Global), None);
    }

    #[test]
    fn test_apply_overrides_unknown_action_does_not_block_valid_overrides() {
        let mut kb = KeyBindings::defaults();
        let mut overrides = HashMap::new();
        overrides.insert(
            "not_an_action".to_string(),
            SingleOrVec::Single("ctrl+k".to_string()),
        );
        overrides.insert("save".to_string(), SingleOrVec::Single("alt+s".to_string()));
        apply_keybinding_overrides(&mut kb, overrides, "test");

        let old_event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        let new_event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        assert_eq!(kb.lookup(&old_event, KeyScope::Global), None);
        assert_eq!(
            kb.lookup(&new_event, KeyScope::Global),
            Some(KeyAction::Save)
        );
    }

    #[test]
    fn test_apply_overrides_invalid_bind_does_not_unbind_default() {
        let mut kb = KeyBindings::defaults();
        let mut overrides = HashMap::new();
        overrides.insert(
            "save".to_string(),
            SingleOrVec::Single("ctrl+notakey".to_string()),
        );
        apply_keybinding_overrides(&mut kb, overrides, "test");

        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(kb.lookup(&event, KeyScope::Global), Some(KeyAction::Save));
    }

    #[test]
    fn test_parse_key_action_name() {
        assert_eq!(parse_key_action_name("save"), Some(KeyAction::Save));
        assert_eq!(parse_key_action_name("no_such_action"), None);
    }

    #[test]
    fn test_selected_action_handles_empty_actions() {
        let actions: Vec<KeyAction> = Vec::new();
        assert_eq!(selected_action(&actions, 0), None);
    }

    #[test]
    fn test_parse_override_entry_tolerates_bad_types_per_entry() {
        let good = parse_override_entry(
            "save",
            serde_json::Value::String("ctrl+s".to_string()),
            "test",
        );
        assert!(good.is_some());

        let bad = parse_override_entry("save", serde_json::json!(123), "test");
        assert!(bad.is_none());
    }

    #[test]
    fn test_parse_override_entry_array_requires_all_strings() {
        let ok = parse_override_entry(
            "redo",
            serde_json::json!(["ctrl+y", "ctrl+shift+z"]),
            "test",
        );
        assert!(ok.is_some());

        let bad = parse_override_entry("redo", serde_json::json!(["ctrl+y", 7]), "test");
        assert!(bad.is_none());
    }

    #[test]
    fn test_conflict_overwrite_replace_semantics() {
        let mut kb = KeyBindings::defaults();
        let target = KeyAction::NewFile;
        let conflict_bind = KeyBind::parse("ctrl+s").unwrap();

        // Simulate overwrite-confirm flow:
        if let Some(conflict_action) = kb.find_conflict(&conflict_bind, target) {
            kb.remove_bind_from(conflict_action, &conflict_bind);
        }
        kb.map.insert(target, vec![conflict_bind.clone()]);

        // Target action should now only have the new bind.
        assert_eq!(
            kb.map.get(&target).cloned(),
            Some(vec![conflict_bind.clone()])
        );
        // Old target bind should be gone.
        let old_evt = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_ne!(kb.lookup(&old_evt, KeyScope::Global), Some(target));
        // New bind should resolve to target action.
        let new_evt = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(kb.lookup(&new_evt, KeyScope::Global), Some(target));
    }
}

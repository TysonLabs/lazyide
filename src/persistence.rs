use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const STATE_FILE_REL: &str = "lazyide/state.json";

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct PersistedState {
    pub(crate) theme_name: String,
    #[serde(default)]
    pub(crate) files_pane_width: Option<u16>,
    #[serde(default)]
    pub(crate) word_wrap: Option<bool>,
}

pub(crate) fn autosave_path_for(path: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();
    let base = state_file_path()
        .and_then(|p| p.parent().map(|pp| pp.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("autosave").join(format!("{hash:016x}.autosave"))
}

pub(crate) fn state_file_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join(STATE_FILE_REL));
    }
    if let Ok(appdata) = std::env::var("APPDATA")
        && !appdata.is_empty()
    {
        return Some(PathBuf::from(appdata).join(STATE_FILE_REL));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config").join(STATE_FILE_REL))
}

pub(crate) fn load_persisted_state() -> Option<PersistedState> {
    let path = state_file_path()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str::<PersistedState>(&raw).ok()
}

pub(crate) fn save_persisted_state(state: &PersistedState) -> io::Result<()> {
    let Some(path) = state_file_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| io::Error::other(format!("serialize state: {e}")))?;
    fs::write(path, raw)
}

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Tree,
    Editor,
}

#[derive(Debug, Clone)]
pub(crate) enum PendingAction {
    None,
    Quit,
    ClosePrompt,
    Delete(PathBuf),
}

#[derive(Debug, Clone)]
pub(crate) enum PromptMode {
    NewFile { parent: PathBuf },
    NewFolder { parent: PathBuf },
    Rename { target: PathBuf },
    FindInFile,
    FindInProject,
    ReplaceInFile { search: String },
    GoToLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandAction {
    Theme,
    Help,
    QuickOpen,
    FindInFile,
    FindInProject,
    SaveFile,
    RefreshTree,
    ToggleFiles,
    GotoDefinition,
    ReplaceInFile,
    GoToLine,
    Keybinds,
    ToggleWordWrap,
}

#[derive(Debug, Clone)]
pub(crate) struct PromptState {
    pub(crate) title: String,
    pub(crate) value: String,
    pub(crate) cursor: usize,
    pub(crate) mode: PromptMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextAction {
    Open,
    NewFile,
    NewFolder,
    Rename,
    Delete,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorContextAction {
    Copy,
    Cut,
    Paste,
    SelectAll,
    Cancel,
}

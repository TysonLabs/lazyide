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
    /// Saving a notebook would change its JSON whitespace; ask first.
    /// `then_close` closes the tab after a confirmed save.
    #[allow(dead_code)] // TEMPORARY: constructed by the notebook save path once it lands.
    NotebookReformat {
        then_close: bool,
    },
}

/// Result of `App::save_file`. `Err` is reserved for I/O errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveOutcome {
    /// The buffer was written to disk.
    Saved,
    /// Nothing was written yet; a prompt (`PendingAction::NotebookReformat`) asks first.
    #[allow(dead_code)] // TEMPORARY: returned by the notebook save path once it lands.
    Deferred,
    /// Nothing was written (no file open, or the buffer could not be saved); the status says why.
    Failed,
}

#[derive(Debug, Clone)]
pub(crate) enum PromptMode {
    NewFile { parent: PathBuf },
    NewFolder { parent: PathBuf },
    Rename { target: PathBuf },
    Move { target: PathBuf },
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
    GitDiff,
    SplitVertical,
    SplitHorizontal,
    FocusOtherPane,
    MoveTabToOtherPane,
    ClosePane,
    ToggleMinimap,
    NotebookNextCell,
    NotebookPrevCell,
    NotebookInsertCellBelow,
    NotebookToggleCellType,
    NotebookClearOutputs,
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
    Move,
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

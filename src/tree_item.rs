use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct TreeItem {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) depth: usize,
    pub(crate) is_dir: bool,
    pub(crate) expanded: bool,
}

use std::path::{Path, PathBuf};

pub const STATIC_ASSET_SOURCE: &str = "python/refine_ui/static";
pub const STATIC_ASSET_DESTINATION: &str = "rust/src/surfaces/web/static";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticAssetTree {
    pub root: PathBuf,
}

impl StaticAssetTree {
    pub fn source_copy(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().join("src/surfaces/web/static"),
        }
    }
}

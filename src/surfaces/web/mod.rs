use std::path::{Path, PathBuf};

pub const STATIC_ASSET_SOURCE: &str = "src/surfaces/web/static";
pub const STATIC_ASSET_DESTINATION: &str = "src/surfaces/web/static";

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

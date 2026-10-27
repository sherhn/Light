//! Canonical storage names for Light.
use std::path::{Path, PathBuf};

/// Project-local history and metadata.
pub fn project_data_dir(root: &Path) -> PathBuf {
    root.join(".light")
}

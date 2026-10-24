//! Storage names for Light, with the previous release's data kept readable in place.
use std::path::{Path, PathBuf};

/// Preserve existing data without moving it; new installations use the current name.
pub fn compatible_dir(parent: &Path, current: &str, previous: &str) -> PathBuf {
    let current = parent.join(current);
    let previous = parent.join(previous);
    if !current.exists() && previous.is_dir() { previous } else { current }
}

pub fn project_data_dir(root: &Path) -> PathBuf {
    compatible_dir(root, ".light", ".air")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn legacy_settings_and_project_data_remain_available_without_moving_them() {
        let temp = TempDir::new("branding-paths");
        for (current, previous) in [("Light", "Air"), (".light", ".air")] {
            assert_eq!(compatible_dir(temp.path(), current, previous), temp.path().join(current));
            std::fs::create_dir(temp.path().join(previous)).unwrap();
            std::fs::write(temp.path().join(previous).join("settings.json"), "retained").unwrap();
            assert_eq!(compatible_dir(temp.path(), current, previous), temp.path().join(previous));
            std::fs::create_dir(temp.path().join(current)).unwrap();
            assert_eq!(compatible_dir(temp.path(), current, previous), temp.path().join(current));
            assert_eq!(std::fs::read_to_string(temp.path().join(previous).join("settings.json")).unwrap(), "retained");
        }
    }
}

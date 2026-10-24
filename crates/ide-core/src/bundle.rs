//! Optional command-line tools shipped alongside an installed Light executable.

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// Append the bundled Git to PATH, keeping the user's existing tools first.
/// The caller applies this before starting threads or child processes.
pub fn tool_path(executable: &Path, current: Option<&OsStr>) -> Option<OsString> {
    let directory = executable.parent()?.join("git").join("cmd");
    if !directory.join("git.exe").is_file() {
        return None;
    }
    let mut paths: Vec<_> = current.map(std::env::split_paths).into_iter().flatten().collect();
    if paths.contains(&directory) {
        return None;
    }
    paths.push(directory);
    std::env::join_paths(paths).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_supplies_git_without_replacing_existing_tools() {
        let temp = crate::testutil::TempDir::new("bundled-git");
        let bin = temp.path().join("git/cmd");
        std::fs::create_dir_all(&bin).unwrap();
        let executable = temp.path().join("light.exe");
        assert!(tool_path(&executable, None).is_none());
        std::fs::write(bin.join("git.exe"), "git").unwrap();
        let user_bin = temp.path().join("my-tools");
        let previous = std::env::join_paths([&user_bin]).unwrap();
        let result = tool_path(&executable, Some(&previous)).unwrap();
        assert_eq!(std::env::split_paths(&result).collect::<Vec<_>>(), [user_bin, bin.clone()]);
        assert!(tool_path(&executable, Some(&result)).is_none());
        assert_eq!(std::env::split_paths(&tool_path(&executable, None).unwrap()).collect::<Vec<_>>(), [bin]);
    }
}

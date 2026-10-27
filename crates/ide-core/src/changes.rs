//! Layout of the changes list of the Commit window: files grouped into "Changes" and "Unversioned Files",
//! optionally under their directories, with collapsible groups and tri-state selection.

use std::collections::HashSet;
use std::path::Path;

use crate::git::{Change, ChangeKind};
use crate::workspace::IgnoreMatcher;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// Files git knows about (modified, added, deleted, ...).
    Changes,
    /// Untracked files.
    Unversioned,
}

impl Group {
    fn key(self) -> &'static str {
        match self {
            Group::Changes => "changes",
            Group::Unversioned => "unversioned",
        }
    }
}

pub fn group_of(c: &Change) -> Group {
    if c.kind == ChangeKind::Untracked { Group::Unversioned } else { Group::Changes }
}

/// Directory of a change relative to the repository root (`""` for the root). An untracked directory entry
/// (`dir/`) lives in the parent of `dir`.
pub fn dir_of(path: &str) -> &str {
    path.trim_end_matches('/').rsplit_once('/').map_or("", |(d, _)| d)
}

/// The key under which the collapsed state of a group (`dir` = `None`) or of a directory inside it is remembered.
pub fn collapse_key(group: Group, dir: Option<&str>) -> String {
    match dir {
        None => format!("@{}", group.key()),
        Some(d) => format!("@{}/{d}", group.key()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Group { group: Group, count: usize, collapsed: bool },
    Dir { group: Group, dir: String, count: usize, collapsed: bool },
    /// `index` points into the changes slice the rows were built from; `nested` files sit under a directory row.
    File { index: usize, nested: bool },
}

/// The visible rows: each non-empty group, its directories (when `by_dir`) and their files, minus collapsed parts.
pub fn rows(changes: &[Change], by_dir: bool, collapsed: &HashSet<String>) -> Vec<Row> {
    let mut out = Vec::new();
    for group in [Group::Changes, Group::Unversioned] {
        let members: Vec<usize> = (0..changes.len()).filter(|&i| group_of(&changes[i]) == group).collect();
        if members.is_empty() {
            continue;
        }
        let group_collapsed = collapsed.contains(&collapse_key(group, None));
        out.push(Row::Group { group, count: members.len(), collapsed: group_collapsed });
        if group_collapsed {
            continue;
        }
        if !by_dir {
            out.extend(members.iter().map(|&index| Row::File { index, nested: false }));
            continue;
        }
        let mut dirs: Vec<&str> = members.iter().map(|&i| dir_of(&changes[i].path)).collect();
        dirs.sort_unstable();
        dirs.dedup();
        for dir in dirs {
            let files: Vec<usize> = members.iter().copied().filter(|&i| dir_of(&changes[i].path) == dir).collect();
            let dir_collapsed = collapsed.contains(&collapse_key(group, Some(dir)));
            out.push(Row::Dir { group, dir: dir.to_string(), count: files.len(), collapsed: dir_collapsed });
            if !dir_collapsed {
                out.extend(files.into_iter().map(|index| Row::File { index, nested: true }));
            }
        }
    }
    out
}

/// Paths of every change in `group` (and, when given, directly in `dir`), regardless of what is collapsed.
/// Drop the new files and directories that `.gitignore` or `.git/info/exclude` excludes: the project tree greys
/// the same files out, so the Commit window must not offer them either, whatever git reports. Files git tracks or
/// has staged are real changes and stay.
pub fn without_ignored(changes: Vec<Change>, root: &Path) -> Vec<Change> {
    let mut ignore = IgnoreMatcher::new(root);
    changes
        .into_iter()
        .filter(|c| {
            let is_new = c.kind == ChangeKind::Untracked || (c.kind == ChangeKind::Added && !c.staged);
            !is_new || !ignore.is_ignored(&root.join(c.path.trim_end_matches('/')), c.path.ends_with('/'))
        })
        .collect()
}

pub fn paths_in<'a>(changes: &'a [Change], group: Group, dir: Option<&str>) -> Vec<&'a str> {
    changes.iter().filter(|c| group_of(c) == group && dir.is_none_or(|d| dir_of(&c.path) == d)).map(|c| c.path.as_str()).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    On,
    Off,
    Mixed,
}

/// Selection state of a set of paths (a group or directory checkbox).
pub fn check_state(checked: &HashSet<String>, paths: &[&str]) -> Check {
    let on = paths.iter().filter(|p| checked.contains(**p)).count();
    if on == 0 {
        Check::Off
    } else if on == paths.len() {
        Check::On
    } else {
        Check::Mixed
    }
}

/// Click on a group / directory checkbox: select everything unless everything already is, then clear.
pub fn toggle_all(checked: &mut HashSet<String>, paths: &[&str]) {
    if check_state(checked, paths) == Check::On {
        for p in paths {
            checked.remove(*p);
        }
    } else {
        checked.extend(paths.iter().map(|p| p.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(path: &str, kind: ChangeKind) -> Change {
        Change { path: path.into(), kind, staged: false, unstaged: true }
    }

    fn sample() -> Vec<Change> {
        vec![ch("README.md", ChangeKind::Modified), ch("src/a.lisp", ChangeKind::Modified), ch("src/b.lisp", ChangeKind::Added), ch("src/deep/c.lisp", ChangeKind::Deleted), ch("notes.txt", ChangeKind::Untracked), ch("tmp/", ChangeKind::Untracked)]
    }

    #[test]
    fn ignored_new_files_are_dropped_but_tracked_and_staged_ones_stay() {
        use crate::testutil::TempDir;
        let d = TempDir::new("changes-ignored");
        std::fs::write(d.path().join(".gitignore"), "*.fasl\r\n/build/\r\nnotes/\r\n!keep.fasl\r\n").unwrap();
        let staged = Change { path: "forced.fasl".into(), kind: ChangeKind::Added, staged: true, unstaged: false };
        let list = vec![
            ch("src/a.lisp", ChangeKind::Untracked),
            ch("src/b.fasl", ChangeKind::Untracked),
            ch("keep.fasl", ChangeKind::Untracked),
            ch("build/", ChangeKind::Untracked),
            ch("notes/", ChangeKind::Untracked),
            ch("tracked.fasl", ChangeKind::Modified),
            ch("src/c.fasl", ChangeKind::Added),
            ch("src/d.lisp", ChangeKind::Added),
            staged.clone(),
        ];
        let paths: Vec<String> = without_ignored(list, d.path()).into_iter().map(|c| c.path).collect();
        assert_eq!(paths, ["src/a.lisp", "keep.fasl", "tracked.fasl", "src/d.lisp", "forced.fasl"]);
    }

    #[test]
    fn directories_are_taken_from_the_path() {
        assert_eq!(dir_of("a.lisp"), "");
        assert_eq!(dir_of("src/a.lisp"), "src");
        assert_eq!(dir_of("src/deep/c.lisp"), "src/deep");
        assert_eq!(dir_of("tmp/"), "");
        assert_eq!(dir_of("src/tmp/"), "src");
    }

    #[test]
    fn rows_group_by_status_then_directory() {
        let changes = sample();
        let r = rows(&changes, true, &HashSet::new());
        assert_eq!(r[0], Row::Group { group: Group::Changes, count: 4, collapsed: false });
        assert_eq!(r[1], Row::Dir { group: Group::Changes, dir: String::new(), count: 1, collapsed: false });
        assert_eq!(r[2], Row::File { index: 0, nested: true });
        assert_eq!(r[3], Row::Dir { group: Group::Changes, dir: "src".into(), count: 2, collapsed: false });
        assert_eq!(r[6], Row::Dir { group: Group::Changes, dir: "src/deep".into(), count: 1, collapsed: false });
        let unv = r.iter().position(|x| matches!(x, Row::Group { group: Group::Unversioned, .. })).unwrap();
        assert_eq!(r[unv], Row::Group { group: Group::Unversioned, count: 2, collapsed: false });
        // directory entries and files of the root share the root row
        assert_eq!(r[unv + 1], Row::Dir { group: Group::Unversioned, dir: String::new(), count: 2, collapsed: false });
    }

    #[test]
    fn flat_rows_list_files_directly_under_their_group() {
        let changes = sample();
        let r = rows(&changes, false, &HashSet::new());
        assert_eq!(r.len(), 2 + changes.len());
        assert!(r.iter().all(|x| !matches!(x, Row::Dir { .. })));
        assert_eq!(r[1], Row::File { index: 0, nested: false });
    }

    #[test]
    fn collapsed_groups_and_directories_hide_their_files() {
        let changes = sample();
        let mut collapsed = HashSet::new();
        collapsed.insert(collapse_key(Group::Changes, Some("src")));
        let r = rows(&changes, true, &collapsed);
        assert!(r.contains(&Row::Dir { group: Group::Changes, dir: "src".into(), count: 2, collapsed: true }));
        assert!(!r.iter().any(|x| matches!(x, Row::File { index: 1, .. } | Row::File { index: 2, .. })));
        collapsed.insert(collapse_key(Group::Unversioned, None));
        let r = rows(&changes, true, &collapsed);
        assert_eq!(r.last(), Some(&Row::Group { group: Group::Unversioned, count: 2, collapsed: true }));
        assert!(rows(&[], true, &collapsed).is_empty());
    }

    #[test]
    fn selection_state_is_tri_state_and_toggles_whole_sets() {
        let changes = sample();
        let src = paths_in(&changes, Group::Changes, Some("src"));
        assert_eq!(src, vec!["src/a.lisp", "src/b.lisp"]);
        let mut checked = HashSet::new();
        assert_eq!(check_state(&checked, &src), Check::Off);
        checked.insert("src/a.lisp".to_string());
        assert_eq!(check_state(&checked, &src), Check::Mixed);
        toggle_all(&mut checked, &src);
        assert_eq!(check_state(&checked, &src), Check::On);
        toggle_all(&mut checked, &src);
        assert_eq!(check_state(&checked, &src), Check::Off);
        assert!(checked.is_empty());
        let all = paths_in(&changes, Group::Changes, None);
        assert_eq!(all.len(), 4);
        assert_eq!(paths_in(&changes, Group::Unversioned, None), vec!["notes.txt", "tmp/"]);
    }
}

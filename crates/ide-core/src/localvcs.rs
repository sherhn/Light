//! Local commits ("checkpoints"): snapshots of the working tree kept outside of git.
//!
//! Use case: you work on top of somebody else's repository without a fork, so you cannot commit,
//! but you want to "reset the diff" to see only your newest changes. A local commit records the
//! content of every file that differs from git `HEAD`; the *base* for diffs, gutter markers and the
//! changes list can then be `HEAD` or any local commit. Everything lives in `<root>/.light/local`
//! and is excluded from git through `.git/info/exclude` (never committed, never shown as a change).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::{Change, ChangeKind, Repo};

const MAX_FILE: u64 = 5 * 1024 * 1024;
const MAX_FILES_NO_GIT: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Base {
    #[default]
    Head,
    Local(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalCommit {
    pub id: u64,
    pub message: String,
    /// Unix seconds.
    pub time: u64,
    /// Files that differ from git HEAD at that moment: relative path -> blob hash (`None` = file absent).
    pub files: BTreeMap<String, Option<String>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Store {
    next_id: u64,
    commits: Vec<LocalCommit>,
    base: Base,
}

#[derive(Clone, Debug)]
pub struct LocalVcs {
    root: PathBuf,
    store: Store,
}

/// 128-bit content hash (two independent FNV-1a runs); stable across versions, no dependency.
pub fn hash_text(s: &str) -> String {
    let (mut a, mut b) = (0xcbf29ce484222325u64, 0x84222325cbf29ce4u64);
    for &byte in s.as_bytes() {
        a ^= byte as u64;
        a = a.wrapping_mul(0x100000001b3);
        b = b.wrapping_add(byte as u64).wrapping_mul(0x9E3779B97F4A7C15);
        b ^= b >> 29;
    }
    format!("{a:016x}{b:016x}{:08x}", s.len() as u32)
}

fn norm(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn read_text(p: &Path) -> Option<String> {
    let meta = fs::metadata(p).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let bytes = fs::read(p).ok()?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl LocalVcs {
    fn dir(root: &Path) -> PathBuf {
        crate::paths::project_data_dir(root).join("local")
    }

    /// Load the store of `root` (an empty one if none exists yet). Nothing is written until the first commit.
    pub fn open(root: &Path) -> LocalVcs {
        let store = fs::read_to_string(Self::dir(root).join("store.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        LocalVcs { root: root.to_path_buf(), store }
    }

    pub fn commits(&self) -> &[LocalCommit] {
        &self.store.commits
    }

    pub fn commit(&self, id: u64) -> Option<&LocalCommit> {
        self.store.commits.iter().find(|c| c.id == id)
    }

    pub fn base(&self) -> &Base {
        &self.store.base
    }

    fn save(&self) -> io::Result<()> {
        let d = Self::dir(&self.root);
        fs::create_dir_all(d.join("blobs"))?;
        let json = serde_json::to_string_pretty(&self.store).map_err(io::Error::other)?;
        let tmp = d.join("store.json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, d.join("store.json"))
    }

    pub fn set_base(&mut self, base: Base) -> io::Result<()> {
        if let Base::Local(id) = base
            && self.commit(id).is_none()
        {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such local commit"));
        }
        self.store.base = base;
        self.save()
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        Self::dir(&self.root).join("blobs").join(hash)
    }

    fn read_blob(&self, hash: &str) -> Option<String> {
        fs::read_to_string(self.blob_path(hash)).ok()
    }

    /// Make git ignore our directory without touching any tracked file.
    pub fn ensure_git_ignored(repo: &Repo) {
        let info = repo.root.join(".git").join("info");
        let exclude = info.join("exclude");
        let existing = fs::read_to_string(&exclude).unwrap_or_default();
        let missing: Vec<_> = [".light", ".air"].into_iter().filter(|name| !existing.lines().any(|line| line.trim().trim_matches('/') == *name)).collect();
        if missing.is_empty() { return; }
        let _ = fs::create_dir_all(&info);
        let mut s = existing;
        if !s.is_empty() && !s.ends_with('\n') { s.push('\n'); }
        for name in missing { s.push_str(&format!("/{name}/\n")); }
        let _ = fs::write(&exclude, s);
    }

    /// Paths that may differ from the base: everything git reports plus (without git) the working tree.
    fn candidate_paths(&self, repo: Option<&Repo>) -> BTreeSet<String> {
        match repo {
            Some(r) => self.expand_dirs(r.status().into_iter().map(|c| c.path).collect()),
            None => crate::workspace::list_files(&self.root, MAX_FILES_NO_GIT).into_iter().map(|p| crate::workspace::relative(&self.root, &p)).collect(),
        }
    }

    /// `git status` reports wholly untracked directories as `dir/`; list their files (bounded).
    fn expand_dirs(&self, paths: Vec<String>) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for p in paths {
            if let Some(dir) = p.strip_suffix('/') {
                for f in crate::workspace::list_files(&self.root.join(dir), MAX_FILES_NO_GIT).into_iter().take(MAX_FILES_NO_GIT) {
                    out.insert(crate::workspace::relative(&self.root, &f));
                }
            } else {
                out.insert(p);
            }
        }
        out
    }

    /// Content of `rel` at git HEAD (None without git / untracked).
    fn head_text(repo: Option<&Repo>, rel: &str) -> Option<String> {
        repo.and_then(|r| r.head_content(rel))
    }

    /// Create a local commit of the current working tree. The new commit becomes the base.
    pub fn create(&mut self, message: &str, repo: Option<&Repo>) -> io::Result<u64> {
        if let Some(r) = repo {
            Self::ensure_git_ignored(r);
        }
        let mut files = BTreeMap::new();
        fs::create_dir_all(Self::dir(&self.root).join("blobs"))?;
        for rel in self.candidate_paths(repo) {
            let abs = self.root.join(&rel);
            if abs.exists() {
                let Some(text) = read_text(&abs) else { continue };
                // files identical to HEAD are not part of the snapshot
                if Self::head_text(repo, &rel).is_some_and(|h| norm(&h) == norm(&text)) {
                    continue;
                }
                let h = hash_text(&text);
                let bp = self.blob_path(&h);
                if !bp.exists() {
                    fs::write(&bp, &text)?;
                }
                files.insert(rel, Some(h));
            } else if Self::head_text(repo, &rel).is_some() {
                files.insert(rel, None); // deleted relative to HEAD
            }
        }
        let id = self.store.next_id + 1;
        self.store.next_id = id;
        let message = if message.trim().is_empty() { format!("Local commit {id}") } else { message.trim().to_string() };
        self.store.commits.push(LocalCommit { id, message, time: now_secs(), files });
        self.store.base = Base::Local(id);
        self.save()?;
        Ok(id)
    }

    /// The text of `rel` in local commit `id` (falling back to HEAD for files the commit does not list).
    pub fn text_in(&self, id: u64, rel: &str, repo: Option<&Repo>) -> Option<String> {
        let c = self.commit(id)?;
        match c.files.get(rel) {
            Some(Some(h)) => self.read_blob(h),
            Some(None) => None,
            None => Self::head_text(repo, rel),
        }
    }

    /// The local commit made right before `id` (`None` for the first one).
    pub fn previous_id(&self, id: u64) -> Option<u64> {
        let i = self.store.commits.iter().position(|c| c.id == id)?;
        i.checked_sub(1).map(|j| self.store.commits[j].id)
    }

    /// The text of `rel` as it was just before local commit `id`: in the previous local commit, or at git HEAD
    /// for the first one.
    pub fn text_before(&self, id: u64, rel: &str, repo: Option<&Repo>) -> Option<String> {
        match self.previous_id(id) {
            Some(prev) => self.text_in(prev, rel, repo),
            None => Self::head_text(repo, rel),
        }
    }

    /// What local commit `id` changed: files whose content differs from the state before the commit.
    pub fn commit_changes(&self, id: u64, repo: Option<&Repo>) -> Vec<Change> {
        let Some(c) = self.commit(id) else { return vec![] };
        let mut paths: BTreeSet<String> = c.files.keys().cloned().collect();
        if let Some(prev) = self.previous_id(id).and_then(|p| self.commit(p)) {
            paths.extend(prev.files.keys().cloned());
        }
        let mut out = Vec::new();
        for rel in paths {
            let before = self.text_before(id, &rel, repo);
            let after = self.text_in(id, &rel, repo);
            let kind = match (&before, &after) {
                (None, None) => continue,
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Deleted,
                (Some(b), Some(a)) => {
                    if norm(b) == norm(a) {
                        continue;
                    }
                    ChangeKind::Modified
                }
            };
            out.push(Change { path: rel, kind, staged: false, unstaged: true });
        }
        out
    }

    /// Put one file back into its state in local commit `id` (deleting it if the commit has no such file).
    pub fn restore_file(&self, id: u64, rel: &str, repo: Option<&Repo>) -> io::Result<()> {
        if self.commit(id).is_none() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such local commit"));
        }
        let abs = self.root.join(rel);
        match self.text_in(id, rel, repo) {
            Some(t) => {
                if let Some(parent) = abs.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&abs, t)
            }
            None => match fs::remove_file(&abs) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        }
    }

    /// Text of `rel` in the current base (HEAD or a local commit).
    pub fn base_text(&self, rel: &str, repo: Option<&Repo>) -> Option<String> {
        match &self.store.base {
            Base::Head => Self::head_text(repo, rel),
            Base::Local(id) => self.text_in(*id, rel, repo),
        }
    }

    /// Changes of the working tree relative to local commit `id`.
    pub fn changes_vs(&self, id: u64, repo: Option<&Repo>) -> Vec<Change> {
        let Some(c) = self.commit(id) else { return vec![] };
        let mut paths = self.candidate_paths(repo);
        paths.extend(c.files.keys().cloned());
        let mut out = Vec::new();
        for rel in paths {
            let work = read_text(&self.root.join(&rel));
            let base = self.text_in(id, &rel, repo);
            let kind = match (&base, &work) {
                (None, None) => continue,
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Deleted,
                (Some(b), Some(w)) => {
                    if norm(b) == norm(w) {
                        continue;
                    }
                    ChangeKind::Modified
                }
            };
            out.push(Change { path: rel, kind, staged: false, unstaged: true });
        }
        out
    }

    /// Put the working tree back into the state of local commit `id` (for the files the commit and
    /// git know about). Returns the number of files written or removed.
    pub fn restore(&self, id: u64, repo: Option<&Repo>) -> io::Result<usize> {
        let c = self.commit(id).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such local commit"))?;
        let mut paths = self.candidate_paths(repo);
        paths.extend(c.files.keys().cloned());
        let mut n = 0;
        for rel in paths {
            let abs = self.root.join(&rel);
            let want = self.text_in(id, &rel, repo);
            let have = read_text(&abs);
            match (want, have) {
                (None, None) => {}
                (None, Some(_)) => {
                    fs::remove_file(&abs)?;
                    n += 1;
                }
                (Some(w), have) => {
                    if have.as_deref().is_none_or(|h| norm(h) != norm(&w)) {
                        if let Some(parent) = abs.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        fs::write(&abs, w)?;
                        n += 1;
                    }
                }
            }
        }
        Ok(n)
    }

    pub fn delete(&mut self, id: u64) -> io::Result<()> {
        self.store.commits.retain(|c| c.id != id);
        if self.store.base == Base::Local(id) {
            self.store.base = Base::Head;
        }
        // drop blobs nobody references any more
        let used: BTreeSet<&String> = self.store.commits.iter().flat_map(|c| c.files.values().flatten()).collect();
        if let Ok(rd) = fs::read_dir(Self::dir(&self.root).join("blobs")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !used.iter().any(|u| **u == name) {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        self.save()
    }

    pub fn rename(&mut self, id: u64, message: &str) -> io::Result<()> {
        if let Some(c) = self.store.commits.iter_mut().find(|c| c.id == id) {
            c.message = message.trim().to_string();
        }
        self.save()
    }
}

/// How long ago a snapshot was made, in the largest fitting unit (so the UI can word it in any language).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    JustNow,
    Minutes(u64),
    Hours(u64),
    Days(u64),
}

pub fn age_of(seconds: u64) -> Age {
    match seconds {
        0..=59 => Age::JustNow,
        60..=3599 => Age::Minutes(seconds / 60),
        3600..=86_399 => Age::Hours(seconds / 3600),
        _ => Age::Days(seconds / 86_400),
    }
}

/// Age of a unix timestamp.
pub fn age(time: u64) -> Age {
    age_of(now_secs().saturating_sub(time))
}

/// "3 min ago" style text for a unix timestamp.
pub fn ago(time: u64) -> String {
    match age(time) {
        Age::JustNow => "just now".into(),
        Age::Minutes(n) => format!("{n} min ago"),
        Age::Hours(n) => format!("{n} h ago"),
        Age::Days(n) => format!("{n} d ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;
    use crate::testutil::TempDir;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn kinds(v: &[Change]) -> Vec<(String, ChangeKind)> {
        v.iter().map(|c| (c.path.clone(), c.kind)).collect()
    }

    #[test]
    fn legacy_checkpoints_are_read_and_restored_after_the_rebrand() {
        let d = TempDir::new("legacy-checkpoints");
        write(d.path(), "main.py", "original\n");
        let mut vcs = LocalVcs::open(d.path());
        let id = vcs.create("before rebrand", None).unwrap();
        fs::rename(d.path().join(".light"), d.path().join(".air")).unwrap();
        write(d.path(), "main.py", "edited\n");
        let mut reopened = LocalVcs::open(d.path());
        assert_eq!(reopened.commit(id).unwrap().message, "before rebrand");
        reopened.restore_file(id, "main.py", None).unwrap();
        assert_eq!(fs::read_to_string(d.path().join("main.py")).unwrap(), "original\n");
        reopened.rename(id, "retained").unwrap();
        assert_eq!(LocalVcs::open(d.path()).commit(id).unwrap().message, "retained");
        assert!(!d.path().join(".light").exists());
    }

    #[test]
    fn hash_is_stable_and_sensitive() {
        assert_eq!(hash_text("abc"), hash_text("abc"));
        assert_ne!(hash_text("abc"), hash_text("abd"));
        assert_ne!(hash_text(""), hash_text("a"));
        assert_eq!(hash_text("hello"), "a430d84680aabd0bd804bb2991e127f800000005", "pinned so stored blobs stay valid across versions");
    }

    #[test]
    fn works_without_git_every_file_is_new_until_committed() {
        let d = TempDir::new("lv1");
        write(d.path(), "a.lisp", "(a)\n");
        write(d.path(), "src/b.lisp", "(b)\n");
        let mut v = LocalVcs::open(d.path());
        assert!(v.commits().is_empty() && *v.base() == Base::Head);
        let id = v.create("first", None).unwrap();
        assert_eq!(*v.base(), Base::Local(id), "a new local commit becomes the base");
        assert!(v.changes_vs(id, None).is_empty(), "nothing changed since the snapshot");
        write(d.path(), "a.lisp", "(a changed)\n");
        write(d.path(), "c.lisp", "(c)\n");
        fs::remove_file(d.path().join("src/b.lisp")).unwrap();
        let ch = kinds(&v.changes_vs(id, None));
        assert!(ch.contains(&("a.lisp".into(), ChangeKind::Modified)), "{ch:?}");
        assert!(ch.contains(&("c.lisp".into(), ChangeKind::Added)), "{ch:?}");
        assert!(ch.contains(&("src/b.lisp".into(), ChangeKind::Deleted)), "{ch:?}");
        assert_eq!(v.base_text("a.lisp", None).as_deref(), Some("(a)\n"));
        assert_eq!(v.base_text("c.lisp", None), None);
    }

    #[test]
    fn checkpoints_reset_the_diff_on_top_of_a_foreign_repo() {
        if !git::git_available() {
            return;
        }
        let d = TempDir::new("lv2");
        let repo = git::init(d.path()).unwrap();
        for (k, v) in [("user.email", "t@example.com"), ("user.name", "T"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            std::process::Command::new("git").args(["config", k, v]).current_dir(d.path()).output().unwrap();
        }
        write(d.path(), "upstream.lisp", "(upstream 1)\n");
        repo.commit("upstream", &["upstream.lisp".into()]).unwrap();

        // my first round of work on top of the foreign repo
        write(d.path(), "upstream.lisp", "(upstream 1)\n(my addition)\n");
        write(d.path(), "mine.lisp", "(mine)\n");
        let mut v = LocalVcs::open(d.path());
        assert_eq!(kinds(&v.changes_vs_head_for_test(&repo)).len(), 2);
        let c1 = v.create("round one", Some(&repo)).unwrap();
        // the diff is reset: nothing differs from the checkpoint
        assert!(v.changes_vs(c1, Some(&repo)).is_empty());
        // git still sees both files as changed against HEAD (we never touched git)
        assert_eq!(repo.status().len(), 2);
        assert!(!repo.status().iter().any(|c| c.path.starts_with(".light")), "our store must be invisible to git");

        // second round: only the new edit shows up against the checkpoint
        write(d.path(), "mine.lisp", "(mine)\n(more)\n");
        let ch = kinds(&v.changes_vs(c1, Some(&repo)));
        assert_eq!(ch, vec![("mine.lisp".to_string(), ChangeKind::Modified)]);
        assert_eq!(v.base_text("mine.lisp", Some(&repo)).as_deref(), Some("(mine)\n"));
        assert_eq!(v.base_text("upstream.lisp", Some(&repo)).as_deref(), Some("(upstream 1)\n(my addition)\n"));

        // switching the base back to HEAD shows the whole diff again
        v.set_base(Base::Head).unwrap();
        assert_eq!(v.base_text("upstream.lisp", Some(&repo)).as_deref(), Some("(upstream 1)\n"));
        assert!(v.set_base(Base::Local(99)).is_err());

        // restore the first checkpoint
        let n = v.restore(c1, Some(&repo)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(fs::read_to_string(d.path().join("mine.lisp")).unwrap(), "(mine)\n");
    }

    #[test]
    fn reverting_a_file_to_head_is_recorded_correctly() {
        if !git::git_available() {
            return;
        }
        let d = TempDir::new("lv3");
        let repo = git::init(d.path()).unwrap();
        for (k, v) in [("user.email", "t@example.com"), ("user.name", "T"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            std::process::Command::new("git").args(["config", k, v]).current_dir(d.path()).output().unwrap();
        }
        write(d.path(), "f.lisp", "(v1)\n");
        repo.commit("c", &["f.lisp".into()]).unwrap();
        write(d.path(), "f.lisp", "(v2)\n");
        let mut v = LocalVcs::open(d.path());
        let c1 = v.create("edit", Some(&repo)).unwrap();
        // back to the HEAD content: relative to the checkpoint that is a change
        write(d.path(), "f.lisp", "(v1)\n");
        assert_eq!(kinds(&v.changes_vs(c1, Some(&repo))), vec![("f.lisp".to_string(), ChangeKind::Modified)]);
        let c2 = v.create("reverted", Some(&repo)).unwrap();
        assert!(v.commit(c2).unwrap().files.is_empty(), "identical to HEAD: nothing to store");
        assert_eq!(v.base_text("f.lisp", Some(&repo)).as_deref(), Some("(v1)\n"));
    }

    #[test]
    fn persistence_delete_and_blob_cleanup() {
        let d = TempDir::new("lv4");
        write(d.path(), "a.lisp", "(a)\n");
        let mut v = LocalVcs::open(d.path());
        let c1 = v.create("one", None).unwrap();
        write(d.path(), "a.lisp", "(a2)\n");
        let c2 = v.create("two", None).unwrap();
        assert_ne!(c1, c2);
        let again = LocalVcs::open(d.path());
        assert_eq!(again.commits().len(), 2);
        assert_eq!(*again.base(), Base::Local(c2));
        assert_eq!(again.commits()[0].message, "one");
        let blobs = || fs::read_dir(d.path().join(".light/local/blobs")).unwrap().count();
        assert_eq!(blobs(), 2);
        v.delete(c2).unwrap();
        assert_eq!(*v.base(), Base::Head, "deleting the base falls back to HEAD");
        assert_eq!(blobs(), 1, "unreferenced blobs are removed");
        v.rename(c1, "renamed").unwrap();
        assert_eq!(LocalVcs::open(d.path()).commits()[0].message, "renamed");
        // the store never snapshots itself
        let c3 = v.create("", None).unwrap();
        assert!(v.commit(c3).unwrap().files.keys().all(|k| !k.contains(".light")));
        assert_eq!(v.commit(c3).unwrap().message, format!("Local commit {c3}"));
    }

    #[test]
    fn restore_deletes_files_created_after_the_checkpoint() {
        let d = TempDir::new("lv5");
        write(d.path(), "a.lisp", "(a)\n");
        let mut v = LocalVcs::open(d.path());
        let c = v.create("base", None).unwrap();
        write(d.path(), "new.lisp", "(new)\n");
        write(d.path(), "a.lisp", "(changed)\n");
        let n = v.restore(c, None).unwrap();
        assert_eq!(n, 2);
        assert!(!d.path().join("new.lisp").exists());
        assert_eq!(fs::read_to_string(d.path().join("a.lisp")).unwrap(), "(a)\n");
    }

    #[test]
    fn crlf_differences_are_not_changes() {
        let d = TempDir::new("lv6");
        write(d.path(), "a.lisp", "(a)\n(b)\n");
        let mut v = LocalVcs::open(d.path());
        let c = v.create("x", None).unwrap();
        write(d.path(), "a.lisp", "(a)\r\n(b)\r\n");
        assert!(v.changes_vs(c, None).is_empty());
    }

    #[test]
    fn commit_changes_show_what_each_commit_did() {
        let d = TempDir::new("lv7");
        write(d.path(), "a.lisp", "(a)\n");
        write(d.path(), "b.lisp", "(b)\n");
        let mut v = LocalVcs::open(d.path());
        let c1 = v.create("one", None).unwrap();
        assert_eq!(v.previous_id(c1), None);
        // the first commit has nothing before it: everything is new
        assert_eq!(kinds(&v.commit_changes(c1, None)), vec![("a.lisp".to_string(), ChangeKind::Added), ("b.lisp".to_string(), ChangeKind::Added)]);
        write(d.path(), "a.lisp", "(a2)\n");
        fs::remove_file(d.path().join("b.lisp")).unwrap();
        write(d.path(), "c.lisp", "(c)\n");
        let c2 = v.create("two", None).unwrap();
        assert_eq!(v.previous_id(c2), Some(c1));
        assert_eq!(
            kinds(&v.commit_changes(c2, None)),
            vec![("a.lisp".to_string(), ChangeKind::Modified), ("b.lisp".to_string(), ChangeKind::Deleted), ("c.lisp".to_string(), ChangeKind::Added)]
        );
        assert_eq!(v.text_before(c2, "a.lisp", None).as_deref(), Some("(a)\n"));
        assert_eq!(v.text_in(c2, "a.lisp", None).as_deref(), Some("(a2)\n"));
        // a commit that changes nothing lists nothing
        let c3 = v.create("three", None).unwrap();
        assert!(v.commit_changes(c3, None).is_empty());
        assert!(v.commit_changes(99, None).is_empty());
    }

    #[test]
    fn restore_one_file_to_a_commit() {
        let d = TempDir::new("lv8");
        write(d.path(), "a.lisp", "(a)\n");
        write(d.path(), "b.lisp", "(b)\n");
        let mut v = LocalVcs::open(d.path());
        let c = v.create("base", None).unwrap();
        write(d.path(), "a.lisp", "(changed)\n");
        write(d.path(), "b.lisp", "(changed)\n");
        write(d.path(), "extra.lisp", "(x)\n");
        v.restore_file(c, "a.lisp", None).unwrap();
        v.restore_file(c, "extra.lisp", None).unwrap();
        assert_eq!(fs::read_to_string(d.path().join("a.lisp")).unwrap(), "(a)\n");
        assert_eq!(fs::read_to_string(d.path().join("b.lisp")).unwrap(), "(changed)\n", "other files are untouched");
        assert!(!d.path().join("extra.lisp").exists(), "a file the commit does not have is removed");
        v.restore_file(c, "extra.lisp", None).unwrap(); // already gone: fine
        assert!(v.restore_file(99, "a.lisp", None).is_err());
    }

    #[test]
    fn ignored_files_inside_a_new_directory_never_become_changes() {
        if !git::git_available() {
            return;
        }
        let d = TempDir::new("lv-ignored-dir");
        let repo = git::init(d.path()).unwrap();
        for (k, v) in [("user.email", "t@example.com"), ("user.name", "T"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            std::process::Command::new("git").args(["config", k, v]).current_dir(d.path()).output().unwrap();
        }
        write(d.path(), ".gitignore", "*.fasl\n/build/\n");
        write(d.path(), "a.lisp", "(a)\n");
        repo.commit("base", &[".gitignore".into(), "a.lisp".into()]).unwrap();
        // git reports the new directory as one `fresh/` entry; its files are listed here, ignore rules included
        write(d.path(), "fresh/keep.lisp", "(keep)\n");
        write(d.path(), "fresh/generated.fasl", "compiled");
        write(d.path(), "fresh/sub/more.lisp", "(more)\n");
        write(d.path(), "build/out.bin", "generated");
        let mut v = LocalVcs::open(d.path());
        let id = v.create("snapshot", Some(&repo)).unwrap();
        let in_snapshot: Vec<_> = v.commit(id).unwrap().files.keys().cloned().collect();
        assert_eq!(in_snapshot, ["fresh/keep.lisp", "fresh/sub/more.lisp"], "{in_snapshot:?}");
        write(d.path(), "fresh/other.fasl", "x");
        write(d.path(), "fresh/new.lisp", "(new)\n");
        let changes: Vec<_> = v.changes_vs(id, Some(&repo)).into_iter().map(|c| c.path).collect();
        assert_eq!(changes, ["fresh/new.lisp"], "{changes:?}");
    }

    #[test]
    fn age_units() {
        assert_eq!(age_of(5), Age::JustNow);
        assert_eq!(age_of(120), Age::Minutes(2));
        assert_eq!(age_of(7200), Age::Hours(2));
        assert_eq!(age_of(3 * 86_400), Age::Days(3));
    }

    #[test]
    fn ago_formats() {
        assert_eq!(ago(now_secs()), "just now");
        assert_eq!(ago(now_secs() - 120), "2 min ago");
        assert_eq!(ago(now_secs() - 7200), "2 h ago");
        assert_eq!(ago(now_secs() - 3 * 86_400), "3 d ago");
    }

    impl LocalVcs {
        /// Test helper: the changes git itself reports.
        fn changes_vs_head_for_test(&self, repo: &Repo) -> Vec<Change> {
            repo.status()
        }
    }
}

//! Git integration through the `git` executable.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    pub fn letter(self) -> &'static str {
        match self {
            ChangeKind::Modified => "M",
            ChangeKind::Added => "A",
            ChangeKind::Deleted => "D",
            ChangeKind::Renamed => "R",
            ChangeKind::Untracked => "?",
            ChangeKind::Conflicted => "!",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Path relative to the repository root, `/` separated.
    pub path: String,
    pub kind: ChangeKind,
    pub staged: bool,
    pub unstaged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub remote: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub when: String,
    pub subject: String,
    pub parents: Vec<String>,
    /// Branch, tag and HEAD decorations (`HEAD -> main`, `origin/main`, `tag: v1`).
    pub refs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Repo {
    pub root: PathBuf,
}

pub fn git_available() -> bool {
    run_in(Path::new("."), &["--version"]).is_ok()
}

/// `git` working in `dir`: the Windows git, or the distribution's own git for a folder on a WSL file system.
pub fn git_command(dir: &Path) -> Command {
    let env = [("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()), ("LC_ALL".to_string(), "C".to_string())];
    match crate::wsl::wsl_path(dir) {
        Some(w) => {
            let mut cmd = crate::wsl::command(&w.distro, Some(&w.linux), "git");
            crate::wsl::pass_env(&mut cmd, &env);
            cmd
        }
        None => {
            let mut cmd = Command::new("git");
            cmd.current_dir(dir).envs(env);
            cmd
        }
    }
}

/// The branch a local branch follows on its remote, and how far apart they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// `origin/main`
    pub name: String,
    /// Commits here that the remote branch does not have (to push).
    pub ahead: usize,
    /// Commits of the remote branch that are not here yet (to pull).
    pub behind: usize,
}

/// `git fetch` that never asks anything: no prompt in a terminal, no window of a credential manager, no passphrase of
/// SSH. Without what it needs it just fails, and it gives up after `timeout`.
fn fetch_in(dir: &Path, timeout: std::time::Duration) -> Result<(), String> {
    let mut cmd = git_command(dir);
    let quiet = [("GCM_INTERACTIVE", "never"), ("SSH_ASKPASS_REQUIRE", "never"), ("GIT_ASKPASS", ""), ("SSH_ASKPASS", "")];
    match crate::wsl::wsl_path(dir) {
        Some(_) => crate::wsl::pass_env(&mut cmd, &quiet.map(|(k, v)| (k.to_string(), v.to_string()))),
        None => {
            cmd.envs(quiet);
        }
    }
    cmd.args(["-c", "credential.interactive=false", "fetch", "--quiet", "--no-tags", "--prune"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_4000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot run git: {e}"))?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut err);
                }
                return Err(err.trim().to_string());
            }
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("git fetch took too long".into());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn run_in(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = git_command(dir);
    cmd.arg("-c").arg("core.quotepath=false").arg("-c").arg("color.ui=false").args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // no console window, below-normal priority so a big `git status` cannot starve the IDE
        cmd.creation_flags(0x0800_0000 | 0x0000_4000);
    }
    let out = cmd.output().map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let o = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Err(if err.is_empty() { o } else { err })
    }
}

/// Find the repository containing `start` by walking up to a `.git` entry.
pub fn find_repo(start: &Path) -> Option<Repo> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return Some(Repo { root: d.to_path_buf() });
        }
        dir = d.parent();
    }
    None
}

/// `git init` in `dir`.
pub fn init(dir: &Path) -> Result<Repo, String> {
    run_in(dir, &["init", "-q"])?;
    find_repo(dir).ok_or_else(|| "git init did not create a repository".to_string())
}

impl Repo {
    /// A `.git` entry exists but may be broken (half-deleted, corrupt); ask git itself.
    pub fn is_valid(&self) -> bool {
        // git may silently use a parent repository when our `.git` is unusable: require the same top level
        let Ok(top) = self.run(&["rev-parse", "--show-toplevel"]) else { return false };
        if let Some(w) = crate::wsl::wsl_path(&self.root) {
            // git inside the distribution answers with a Linux path
            return top.trim().trim_end_matches('/') == w.linux.trim_end_matches('/') || (w.linux == "/" && top.trim() == "/");
        }
        let norm = |p: &Path| {
            let c = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            c.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/").to_lowercase()
        };
        norm(Path::new(top.trim())) == norm(&self.root)
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        run_in(&self.root, args)
    }

    pub fn branch(&self) -> Option<String> {
        let b = self.run(&["rev-parse", "--abbrev-ref", "HEAD"]).ok()?.trim().to_string();
        if b == "HEAD" {
            // detached or no commits yet
            return self.run(&["rev-parse", "--short", "HEAD"]).ok().map(|s| s.trim().to_string()).or_else(|| self.run(&["symbolic-ref", "--short", "HEAD"]).ok().map(|s| s.trim().to_string()));
        }
        (!b.is_empty()).then_some(b)
    }

    pub fn head_hash(&self) -> Option<String> {
        self.run(&["rev-parse", "HEAD"]).ok().map(|s| s.trim().to_string())
    }

    pub fn status(&self) -> Vec<Change> {
        // `normal` collapses wholly untracked directories into one `dir/` entry; `all` takes minutes on big trees
        let Ok(out) = self.run(&["--no-optional-locks", "status", "--porcelain=v1", "-z", "--untracked-files=normal"]) else { return vec![] };
        let mut v = Vec::new();
        let mut parts = out.split('\0').filter(|s| !s.is_empty());
        while let Some(rec) = parts.next() {
            if rec.len() < 4 {
                continue;
            }
            let (x, y) = (rec.as_bytes()[0] as char, rec.as_bytes()[1] as char);
            let path = rec[3..].to_string();
            if x == 'R' || x == 'C' {
                parts.next(); // original path
            }
            let kind = if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
                ChangeKind::Conflicted
            } else if x == '?' {
                ChangeKind::Untracked
            } else if x == 'R' || y == 'R' {
                ChangeKind::Renamed
            } else if x == 'A' || y == 'A' {
                ChangeKind::Added
            } else if x == 'D' || y == 'D' {
                ChangeKind::Deleted
            } else {
                ChangeKind::Modified
            };
            v.push(Change { path, kind, staged: x != ' ' && x != '?', unstaged: y != ' ' || x == '?' });
        }
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    pub fn branches(&self) -> Vec<Branch> {
        let Ok(out) = self.run(&["branch", "-a", "--format=%(HEAD)|%(refname:short)"]) else { return vec![] };
        out.lines()
            .filter_map(|l| {
                let (h, name) = l.split_once('|')?;
                if name.ends_with("/HEAD") || name.contains("HEAD detached") || name.starts_with('(') {
                    return None;
                }
                Some(Branch { name: name.to_string(), current: h == "*", remote: name.contains('/') && name.split('/').next().is_some_and(|r| self.is_remote(r)) })
            })
            .collect()
    }

    fn is_remote(&self, name: &str) -> bool {
        self.run(&["remote"]).map(|o| o.lines().any(|l| l == name)).unwrap_or(false)
    }

    pub fn has_remote(&self) -> bool {
        self.run(&["remote"]).map(|o| !o.trim().is_empty()).unwrap_or(false)
    }

    /// Content of `rel` at HEAD (None for untracked files / no commits).
    pub fn head_content(&self, rel: &str) -> Option<String> {
        self.run(&["show", &format!("HEAD:{rel}")]).ok()
    }

    pub fn rel(&self, p: &Path) -> Option<String> {
        p.strip_prefix(&self.root).ok().map(|r| r.to_string_lossy().replace('\\', "/"))
    }

    /// The newest `n` commits of the current branch.
    pub fn log(&self, n: usize) -> Vec<Commit> {
        self.log_with(n, false)
    }

    /// The newest `n` commits of every branch, in the order a graph needs (children before parents).
    pub fn log_all(&self, n: usize) -> Vec<Commit> {
        self.log_with(n, true)
    }

    fn log_with(&self, n: usize, all: bool) -> Vec<Commit> {
        let fmt = "--format=%H%x1f%h%x1f%an%x1f%ar%x1f%P%x1f%D%x1f%s";
        let count = format!("-n{n}");
        let mut args = vec!["log", &count, "--topo-order", fmt];
        if all {
            args.insert(1, "--all");
        }
        let Ok(out) = self.run(&args) else { return vec![] };
        out.lines()
            .filter_map(|l| {
                let mut p = l.split('\u{1f}');
                let hash: String = p.next()?.into();
                let short = p.next()?.into();
                let author = p.next()?.into();
                let when = p.next()?.into();
                let parents = p.next()?.split_whitespace().map(String::from).collect();
                let refs = p.next()?.split(", ").filter(|r| !r.is_empty()).map(String::from).collect();
                Some(Commit { hash, short, author, when, subject: p.next().unwrap_or("").into(), parents, refs })
            })
            .collect()
    }

    pub fn show(&self, hash: &str) -> Result<String, String> {
        self.run(&["show", "--stat", "--format=%h  %an  %ar%n%n%B", hash])
    }

    pub fn stage(&self, paths: &[String]) -> Result<(), String> {
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.run(&args).map(|_| ())
    }

    pub fn unstage(&self, paths: &[String]) -> Result<(), String> {
        let mut args = vec!["reset", "-q", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.run(&args).map(|_| ())
    }

    /// Commit exactly `paths` (staging them first); other staged changes stay out of this commit only if
    /// they were not already staged, so callers normally pass the full selection.
    pub fn commit(&self, message: &str, paths: &[String]) -> Result<String, String> {
        if message.trim().is_empty() {
            return Err("Commit message is empty".into());
        }
        if paths.is_empty() {
            return Err("Nothing selected to commit".into());
        }
        self.stage(paths)?;
        let mut args = vec!["commit", "-q", "-m", message, "--"];
        args.extend(paths.iter().map(String::as_str));
        self.run(&args)?;
        Ok(self.run(&["rev-parse", "--short", "HEAD"]).unwrap_or_default().trim().to_string())
    }

    pub fn checkout(&self, branch: &str) -> Result<(), String> {
        self.run(&["checkout", "-q", branch]).map(|_| ())
    }

    pub fn create_branch(&self, name: &str) -> Result<(), String> {
        self.run(&["checkout", "-q", "-b", name]).map(|_| ())
    }

    /// Discard local changes to a tracked file; untracked files are deleted.
    pub fn rollback(&self, change: &Change) -> Result<(), String> {
        if change.kind == ChangeKind::Untracked {
            let p = self.root.join(change.path.trim_end_matches('/'));
            return if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }.map_err(|e| e.to_string());
        }
        if change.staged {
            self.unstage(std::slice::from_ref(&change.path))?;
        }
        if change.kind == ChangeKind::Added {
            return std::fs::remove_file(self.root.join(&change.path)).map_err(|e| e.to_string());
        }
        self.run(&["checkout", "-q", "HEAD", "--", &change.path]).map(|_| ())
    }

    /// The branch the current one follows on its remote and how far apart they are, as of the last fetch.
    pub fn upstream(&self) -> Option<Upstream> {
        let name = self.run(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]).ok()?.trim().to_string();
        let counts = self.run(&["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).ok()?;
        let mut parts = counts.split_whitespace().map(|n| n.parse::<usize>().ok());
        let (ahead, behind) = (parts.next()??, parts.next()??);
        (!name.is_empty()).then_some(Upstream { name, ahead, behind })
    }

    /// Ask the remotes what they have now, without a question to anyone (see [`fetch_in`]).
    pub fn fetch_quietly(&self, timeout: std::time::Duration) -> Result<(), String> {
        fetch_in(&self.root, timeout)
    }

    pub fn pull(&self) -> Result<String, String> {
        self.run(&["pull", "--ff-only"]).map(|s| s.trim().to_string())
    }

    pub fn push(&self) -> Result<String, String> {
        let b = self.branch().ok_or("no current branch")?;
        self.run(&["push", "-u", "origin", &b]).map(|s| s.trim().to_string())
    }

    pub fn diff_text(&self, path: &str) -> String {
        self.run(&["diff", "HEAD", "--", path]).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn repo() -> Option<(TempDir, Repo)> {
        if !git_available() {
            return None;
        }
        let d = TempDir::new("git");
        let r = init(d.path()).ok()?;
        r.run(&["config", "user.email", "t@example.com"]).ok()?;
        r.run(&["config", "user.name", "Tester"]).ok()?;
        r.run(&["config", "commit.gpgsign", "false"]).ok()?;
        Some((d, r))
    }

    #[test]
    fn status_commit_log_and_head_content() {
        let Some((d, r)) = repo() else { return };
        std::fs::write(d.path().join("a.lisp"), "(a)\n").unwrap();
        std::fs::write(d.path().join("b.lisp"), "(b)\n").unwrap();
        let st = r.status();
        assert_eq!(st.len(), 2);
        assert!(st.iter().all(|c| c.kind == ChangeKind::Untracked && c.unstaged && !c.staged));
        assert_eq!(r.commit("", &["a.lisp".into()]), Err("Commit message is empty".into()));
        let short = r.commit("first", &["a.lisp".into(), "b.lisp".into()]).unwrap();
        assert!(!short.is_empty());
        assert!(r.status().is_empty());
        assert_eq!(r.head_content("a.lisp").as_deref(), Some("(a)\n"));
        assert_eq!(r.head_content("nope.lisp"), None);
        let log = r.log(5);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].subject, "first");
        assert_eq!(log[0].author, "Tester");
        assert!(r.head_hash().is_some());
        assert!(r.show(&log[0].hash).unwrap().contains("first"));
    }

    #[test]
    fn modifications_deletions_and_rollback() {
        let Some((d, r)) = repo() else { return };
        std::fs::write(d.path().join("a.lisp"), "one\n").unwrap();
        std::fs::write(d.path().join("b.lisp"), "two\n").unwrap();
        r.commit("init", &["a.lisp".into(), "b.lisp".into()]).unwrap();
        std::fs::write(d.path().join("a.lisp"), "changed\n").unwrap();
        std::fs::remove_file(d.path().join("b.lisp")).unwrap();
        std::fs::write(d.path().join("c.lisp"), "new\n").unwrap();
        let st = r.status();
        let kind = |p: &str| st.iter().find(|c| c.path == p).unwrap().kind;
        assert_eq!(kind("a.lisp"), ChangeKind::Modified);
        assert_eq!(kind("b.lisp"), ChangeKind::Deleted);
        assert_eq!(kind("c.lisp"), ChangeKind::Untracked);
        assert!(r.diff_text("a.lisp").contains("+changed"));
        let a = st.iter().find(|c| c.path == "a.lisp").unwrap().clone();
        r.rollback(&a).unwrap();
        assert_eq!(std::fs::read_to_string(d.path().join("a.lisp")).unwrap().replace("\r\n", "\n"), "one\n");
        let c = st.iter().find(|c| c.path == "c.lisp").unwrap().clone();
        r.rollback(&c).unwrap();
        assert!(!d.path().join("c.lisp").exists());
        r.stage(&["b.lisp".into()]).unwrap();
        assert!(r.status().iter().any(|c| c.path == "b.lisp" && c.staged));
        r.unstage(&["b.lisp".into()]).unwrap();
        assert!(r.status().iter().any(|c| c.path == "b.lisp" && !c.staged && c.unstaged));
    }

    #[test]
    fn branches_checkout_and_create() {
        let Some((d, r)) = repo() else { return };
        std::fs::write(d.path().join("a.lisp"), "x\n").unwrap();
        r.commit("init", &["a.lisp".into()]).unwrap();
        let first = r.branch().unwrap();
        r.create_branch("feature/x").unwrap();
        assert_eq!(r.branch().as_deref(), Some("feature/x"));
        let bs = r.branches();
        assert!(bs.iter().any(|b| b.name == "feature/x" && b.current && !b.remote));
        assert!(bs.iter().any(|b| b.name == first && !b.current));
        r.checkout(&first).unwrap();
        assert_eq!(r.branch().as_deref(), Some(first.as_str()));
        assert!(r.checkout("does-not-exist").is_err());
        assert!(!r.has_remote());
    }

    #[test]
    fn log_all_has_parents_refs_and_every_branch() {
        let Some((d, r)) = repo() else { return };
        std::fs::write(d.path().join("a.lisp"), "x
").unwrap();
        r.commit("base", &["a.lisp".into()]).unwrap();
        let main = r.branch().unwrap();
        r.create_branch("feature").unwrap();
        std::fs::write(d.path().join("f.lisp"), "f
").unwrap();
        r.commit("on feature", &["f.lisp".into()]).unwrap();
        r.checkout(&main).unwrap();
        // the current branch alone does not see the feature commit, the all-branches log does
        assert_eq!(r.log(10).len(), 1);
        let all = r.log_all(10);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].subject, "on feature");
        assert_eq!(all[0].parents, vec![all[1].hash.clone()]);
        assert!(all[0].refs.iter().any(|x| x == "feature"));
        assert!(all[1].refs.iter().any(|x| x.starts_with("HEAD -> ")));
        assert!(all[1].parents.is_empty());
    }

    #[test]
    fn find_repo_walks_up() {
        let Some((d, r)) = repo() else { return };
        std::fs::create_dir_all(d.path().join("sub/deeper")).unwrap();
        let found = find_repo(&d.path().join("sub/deeper")).unwrap();
        assert_eq!(found.root, r.root);
        assert_eq!(r.rel(&d.path().join("sub/x.lisp")).as_deref(), Some("sub/x.lisp"));
    }

    #[test]
    fn spaces_and_unicode_paths() {
        let Some((d, r)) = repo() else { return };
        std::fs::write(d.path().join("my file.lisp"), "x\n").unwrap();
        std::fs::write(d.path().join("файл.lisp"), "y\n").unwrap();
        let st = r.status();
        assert!(st.iter().any(|c| c.path == "my file.lisp"));
        assert!(st.iter().any(|c| c.path == "файл.lisp"), "{st:?}");
        r.commit("m", &["my file.lisp".into(), "файл.lisp".into()]).unwrap();
        assert!(r.status().is_empty());
    }

    #[test]
    fn push_without_remote_reports_error() {
        let Some((d, r)) = repo() else { return };
        std::fs::write(d.path().join("a"), "x").unwrap();
        r.commit("c", &["a".into()]).unwrap();
        assert!(r.push().is_err());
    }

    #[test]
    fn a_fetch_tells_how_far_behind_and_ahead_the_branch_is() {
        let Some((d, r)) = repo() else { return };
        std::fs::write(d.path().join("a"), "1").unwrap();
        r.commit("one", &["a".into()]).unwrap();
        assert_eq!(r.upstream(), None, "no remote yet");
        // a remote, and a second clone of it that goes ahead
        let remote = TempDir::new("gitremote");
        run_in(remote.path(), &["init", "-q", "--bare"]).unwrap();
        let url = remote.path().to_string_lossy().replace('\\', "/");
        r.run(&["remote", "add", "origin", &url]).unwrap();
        let branch = r.branch().unwrap();
        r.run(&["push", "-q", "-u", "origin", &branch]).unwrap();
        assert_eq!(r.upstream().map(|u| (u.ahead, u.behind)), Some((0, 0)));
        assert!(r.upstream().unwrap().name.ends_with(&format!("origin/{branch}")));
        let other = TempDir::new("gitother");
        run_in(other.path(), &["clone", "-q", &url, "."]).unwrap();
        let o = Repo { root: other.path().to_path_buf() };
        for (k, v) in [("user.email", "o@example.com"), ("user.name", "Other"), ("commit.gpgsign", "false")] {
            o.run(&["config", k, v]).unwrap();
        }
        std::fs::write(other.path().join("b"), "2").unwrap();
        o.commit("two", &["b".into()]).unwrap();
        o.run(&["push", "-q"]).unwrap();
        // nothing is known before a fetch
        assert_eq!(r.upstream().map(|u| u.behind), Some(0));
        r.fetch_quietly(std::time::Duration::from_secs(60)).unwrap();
        assert_eq!(r.upstream().map(|u| (u.ahead, u.behind)), Some((0, 1)));
        std::fs::write(d.path().join("c"), "3").unwrap();
        r.commit("three", &["c".into()]).unwrap();
        assert_eq!(r.upstream().map(|u| (u.ahead, u.behind)), Some((1, 1)));
        // a remote that is not there fails without asking anything
        r.run(&["remote", "set-url", "origin", "https://127.0.0.1:9/nothing.git"]).unwrap();
        assert!(r.fetch_quietly(std::time::Duration::from_secs(60)).is_err());
    }
}

#[cfg(test)]
mod validity_tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn broken_dot_git_is_not_a_repository() {
        if !git_available() {
            return;
        }
        let d = TempDir::new("gitbroken");
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        let r = find_repo(d.path()).expect("a .git entry is found");
        assert!(!r.is_valid(), "an empty .git directory is not a usable repository");
        let good_dir = TempDir::new("gitok");
        let ok = init(good_dir.path()).unwrap();
        assert!(ok.is_valid());
    }
}

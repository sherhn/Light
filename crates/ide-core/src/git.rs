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
        Err(command_error(&err, &o))
    }
}

/// A WSL startup warning on stderr must not hide Git diagnostics on stdout.
fn command_error(stderr: &str, stdout: &str) -> String {
    [stderr.trim(), stdout.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
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

    /// Does git itself leave `rel` out of version control because of an ignore rule: `.gitignore` files, the
    /// repository's `.git/info/exclude`, and the user's global ignore file (`core.excludesFile`), which the project's
    /// own rules know nothing of? A file git tracks is never ignored, whatever the rules say.
    pub fn check_ignored(&self, rel: &str) -> bool {
        self.run(&["check-ignore", "-q", "--", rel]).is_ok()
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

    /// The operation that stopped for conflicts and waits to be continued or aborted, if any.
    pub fn merge_state(&self) -> Option<MergeKind> {
        let exists = |name: &str| self.run(&["rev-parse", "--git-path", name]).ok().map(|p| p.trim().to_string()).is_some_and(|p| {
            let path = Path::new(&p);
            let path = if path.is_absolute() { path.to_path_buf() } else { self.root.join(path) };
            match crate::wsl::wsl_path(&self.root) {
                // git in the distribution answers with Linux paths
                Some(w) if p.starts_with('/') => self.root.join(p.strip_prefix(w.linux.trim_end_matches('/')).unwrap_or(&p).trim_start_matches('/')).exists(),
                _ => path.exists(),
            }
        });
        if exists("rebase-merge") || exists("rebase-apply") {
            Some(MergeKind::Rebase)
        } else if exists("MERGE_HEAD") {
            Some(MergeKind::Merge)
        } else if exists("CHERRY_PICK_HEAD") {
            Some(MergeKind::CherryPick)
        } else if exists("REVERT_HEAD") {
            Some(MergeKind::Revert)
        } else {
            None
        }
    }

    /// Merge `branch` into the current branch. Conflicts are an error that names them; the merge then waits for them
    /// to be resolved ([`Repo::merge_state`]).
    pub fn merge(&self, branch: &str) -> Result<String, String> {
        match self.run(&["merge", "--no-edit", branch]) {
            Ok(out) => Ok(out.lines().last().unwrap_or_default().trim().to_string()),
            Err(e) => {
                let conflicted: Vec<String> = self.status().into_iter().filter(|c| c.kind == ChangeKind::Conflicted).map(|c| c.path).collect();
                if conflicted.is_empty() { Err(e) } else { Err(format!("conflicts in {}", conflicted.join(", "))) }
            }
        }
    }

    /// Resolve the conflicts of `path` by taking one side as a whole: `ours` (the current branch) or theirs.
    pub fn take_side(&self, path: &str, ours: bool) -> Result<(), String> {
        self.run(&["checkout", if ours { "--ours" } else { "--theirs" }, "--", path])?;
        self.stage(&[path.to_string()])
    }

    /// The version of a conflicted file on one side: 1 the common base, 2 ours, 3 theirs.
    pub fn stage_content(&self, path: &str, stage: u8) -> Option<String> {
        self.run(&["show", &format!(":{stage}:{path}")]).ok()
    }

    /// Go on with the stopped operation once its conflicts are resolved (a merge is committed with its own message).
    pub fn merge_continue(&self, kind: MergeKind) -> Result<String, String> {
        let args: &[&str] = match kind {
            MergeKind::Merge => &["commit", "--no-edit"],
            MergeKind::Rebase => &["rebase", "--continue"],
            MergeKind::CherryPick => &["cherry-pick", "--continue"],
            MergeKind::Revert => &["revert", "--continue"],
        };
        self.run_quiet_editor(args).map(|s| s.lines().next().unwrap_or_default().trim().to_string())
    }

    /// Give the stopped operation up: the files are as they were before it.
    pub fn merge_abort(&self, kind: MergeKind) -> Result<(), String> {
        let verb = match kind {
            MergeKind::Merge => "merge",
            MergeKind::Rebase => "rebase",
            MergeKind::CherryPick => "cherry-pick",
            MergeKind::Revert => "revert",
        };
        self.run(&[verb, "--abort"]).map(|_| ())
    }

    /// `git` that may want an editor for a message: the message it proposes is taken as it is.
    fn run_quiet_editor(&self, args: &[&str]) -> Result<String, String> {
        let mut full = vec!["-c", "core.editor=true"];
        full.extend_from_slice(args);
        let mut cmd = git_command(&self.root);
        cmd.env("GIT_EDITOR", "true").arg("-c").arg("color.ui=false").args(&full).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if crate::wsl::wsl_path(&self.root).is_some() {
            crate::wsl::pass_env(&mut cmd, &[("GIT_EDITOR".to_string(), "true".to_string())]);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000 | 0x0000_4000);
        }
        let out = cmd.output().map_err(|e| format!("cannot run git: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if out.status.success() { Ok(text) } else { Err(command_error(&err, &text)) }
    }
}

/// Who last changed a line, and in which commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    /// Seconds since the epoch, of the author's commit.
    pub time: i64,
    pub summary: String,
}

impl BlameLine {
    /// A line that is not committed yet.
    pub fn uncommitted(&self) -> bool {
        self.hash.chars().all(|c| c == '0')
    }
}

/// A stash entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stash {
    /// `stash@{0}`
    pub name: String,
    pub message: String,
    pub when: String,
}

/// What an interactive rebase does with a commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebaseAction {
    Pick,
    /// Keep it with a new message.
    Reword,
    /// Melt it into the commit before it, with both messages.
    Squash,
    /// Melt it into the commit before it, keeping that one's message.
    Fixup,
    Drop,
}

/// A commit of an interactive rebase, in the order the commits are to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebaseStep {
    pub hash: String,
    pub action: RebaseAction,
    /// The new message of a reworded commit.
    pub message: String,
}

impl Repo {
    /// Who last changed every line of `rel`, as committed (line `n` of the result is line `n + 1` of the file).
    pub fn blame(&self, rel: &str) -> Result<Vec<BlameLine>, String> {
        let out = self.run(&["blame", "--porcelain", "--", rel])?;
        let mut commits: std::collections::HashMap<String, (String, i64, String)> = std::collections::HashMap::new();
        let mut lines = Vec::new();
        let mut current = String::new();
        for line in out.lines() {
            if let Some(content) = line.strip_prefix('\t') {
                let _ = content;
                let (author, time, summary) = commits.get(&current).cloned().unwrap_or_default();
                lines.push(BlameLine { hash: current.clone(), author, time, summary });
            } else if let Some(rest) = line.strip_prefix("author ") {
                commits.entry(current.clone()).or_default().0 = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("author-time ") {
                commits.entry(current.clone()).or_default().1 = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("summary ") {
                commits.entry(current.clone()).or_default().2 = rest.to_string();
            } else if line.len() >= 40 && line[..40].chars().all(|c| c.is_ascii_hexdigit()) {
                current = line[..40].to_string();
            }
        }
        Ok(lines)
    }

    /// The newest `n` commits that changed `rel`, following it through renames.
    pub fn file_log(&self, rel: &str, n: usize) -> Vec<Commit> {
        let fmt = "--format=%H%x1f%h%x1f%an%x1f%ar%x1f%P%x1f%D%x1f%s";
        let count = format!("-n{n}");
        let Ok(out) = self.run(&["log", &count, "--follow", fmt, "--", rel]) else { return vec![] };
        parse_log(&out)
    }

    /// The content of `rel` in commit `rev` (`None` when it is not there).
    pub fn file_at(&self, rev: &str, rel: &str) -> Option<String> {
        self.run(&["show", &format!("{rev}:{rel}")]).ok()
    }

    /// The path `rel` had in commit `rev` (it may have been renamed since).
    pub fn path_at(&self, rev: &str, rel: &str) -> String {
        self.run(&["log", "-1", "--follow", "--name-only", "--format=", rev, "--", rel]).ok().and_then(|o| o.lines().find(|l| !l.trim().is_empty()).map(String::from)).unwrap_or_else(|| rel.to_string())
    }

    pub fn stashes(&self) -> Vec<Stash> {
        let Ok(out) = self.run(&["stash", "list", "--format=%gd%x1f%s%x1f%cr"]) else { return vec![] };
        out.lines().filter_map(|l| {
            let mut p = l.split('\u{1f}');
            Some(Stash { name: p.next()?.to_string(), message: p.next().unwrap_or_default().to_string(), when: p.next().unwrap_or_default().to_string() })
        }).collect()
    }

    /// Put the changes aside (`untracked`: the new files too); the working tree is as at HEAD after it.
    pub fn stash_save(&self, message: &str, untracked: bool) -> Result<String, String> {
        let mut args = vec!["stash", "push"];
        if untracked {
            args.push("--include-untracked");
        }
        if !message.trim().is_empty() {
            args.extend(["-m", message.trim()]);
        }
        let out = self.run(&args)?;
        if out.contains("No local changes to save") { Err("No local changes to save".into()) } else { Ok(out.trim().to_string()) }
    }

    /// `apply`, `pop` or `drop` the stash `name`.
    pub fn stash_do(&self, verb: &str, name: &str) -> Result<String, String> {
        if !matches!(verb, "apply" | "pop" | "drop") {
            return Err(format!("unknown stash action {verb}"));
        }
        self.run(&["stash", verb, name]).map(|s| s.lines().last().unwrap_or_default().trim().to_string())
    }

    /// Put the changes of commit `hash` on the current branch. Conflicts stop it ([`MergeKind::CherryPick`]).
    pub fn cherry_pick(&self, hash: &str) -> Result<String, String> {
        self.run_quiet_editor(&["cherry-pick", hash]).map(|s| s.lines().next().unwrap_or_default().trim().to_string())
    }

    /// A new commit that undoes commit `hash`. Conflicts stop it ([`MergeKind::Revert`]).
    pub fn revert(&self, hash: &str) -> Result<String, String> {
        self.run_quiet_editor(&["revert", "--no-edit", hash]).map(|s| s.lines().next().unwrap_or_default().trim().to_string())
    }

    /// The commits of `other` that the current branch does not have, and the other way round, the newest first.
    pub fn compare(&self, other: &str, n: usize) -> (Vec<Commit>, Vec<Commit>) {
        let fmt = "--format=%H%x1f%h%x1f%an%x1f%ar%x1f%P%x1f%D%x1f%s";
        let count = format!("-n{n}");
        let theirs = self.run(&["log", &count, fmt, &format!("HEAD..{other}")]).map(|o| parse_log(&o)).unwrap_or_default();
        let ours = self.run(&["log", &count, fmt, &format!("{other}..HEAD")]).map(|o| parse_log(&o)).unwrap_or_default();
        (theirs, ours)
    }

    /// The files that differ between the current branch and `other` (since they parted): (path, change).
    pub fn compare_files(&self, other: &str) -> Vec<(String, ChangeKind)> {
        let Ok(out) = self.run(&["diff", "--name-status", "-z", &format!("HEAD...{other}")]) else { return vec![] };
        let mut parts = out.split('\0').filter(|s| !s.is_empty());
        let mut v = Vec::new();
        while let Some(status) = parts.next() {
            let kind = match status.chars().next() {
                Some('A') => ChangeKind::Added,
                Some('D') => ChangeKind::Deleted,
                Some('R') | Some('C') => {
                    parts.next();
                    ChangeKind::Renamed
                }
                _ => ChangeKind::Modified,
            };
            if let Some(path) = parts.next() {
                v.push((path.to_string(), kind));
            }
        }
        v
    }

    /// The commits after `base` up to HEAD, the oldest first (what an interactive rebase onto `base` goes through).
    pub fn commits_since(&self, base: &str) -> Vec<Commit> {
        let fmt = "--format=%H%x1f%h%x1f%an%x1f%ar%x1f%P%x1f%D%x1f%s";
        self.run(&["log", "--reverse", "--topo-order", fmt, &format!("{base}..HEAD")]).map(|o| parse_log(&o)).unwrap_or_default()
    }

    /// Rebase the current branch onto `base`, doing with every commit what `steps` say, in their order. A reword gets
    /// its new message without an editor; conflicts stop it ([`MergeKind::Rebase`]).
    pub fn rebase_interactive(&self, base: &str, steps: &[RebaseStep]) -> Result<String, String> {
        if steps.first().is_some_and(|s| matches!(s.action, RebaseAction::Squash | RebaseAction::Fixup)) {
            return Err("the first commit cannot be melted into the one before it".into());
        }
        // the list git would put in an editor, written beforehand; its "editor" copies it over git's own
        let git_path = |name: &str| self.run(&["rev-parse", "--git-path", name]).map(|p| p.trim().to_string());
        let todo_rel = git_path("light-rebase-todo")?;
        let local = |rel: &str| match crate::wsl::wsl_path(&self.root) {
            Some(w) if rel.starts_with('/') => self.root.join(rel.strip_prefix(w.linux.trim_end_matches('/')).unwrap_or(rel).trim_start_matches('/')),
            _ if Path::new(rel).is_absolute() => PathBuf::from(rel),
            _ => self.root.join(rel),
        };
        let mut todo = String::new();
        for (n, s) in steps.iter().enumerate() {
            let verb = match s.action {
                RebaseAction::Pick | RebaseAction::Reword => "pick",
                RebaseAction::Squash => "squash",
                RebaseAction::Fixup => "fixup",
                RebaseAction::Drop => "drop",
            };
            todo.push_str(&format!("{verb} {}\n", s.hash));
            if s.action == RebaseAction::Reword {
                let msg_rel = git_path(&format!("light-rebase-message-{n}"))?;
                std::fs::write(local(&msg_rel), &s.message).map_err(|e| e.to_string())?;
                todo.push_str(&format!("exec git commit --amend --only --allow-empty -q -F '{}'\n", msg_rel.replace('\'', "")));
            }
        }
        std::fs::write(local(&todo_rel), todo).map_err(|e| e.to_string())?;
        let editor = format!("cp '{}'", todo_rel.replace('\'', ""));
        let result = self.run_quiet_editor(&["-c", &format!("sequence.editor={editor}"), "rebase", "-i", base]);
        let _ = std::fs::remove_file(local(&todo_rel));
        for n in 0..steps.len() {
            if let Ok(m) = git_path(&format!("light-rebase-message-{n}")) {
                let _ = std::fs::remove_file(local(&m));
            }
        }
        result.map(|s| s.lines().last().unwrap_or_default().trim().to_string())
    }
}

fn parse_log(out: &str) -> Vec<Commit> {
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

/// An operation of Git that can stop for conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeKind {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

#[cfg(test)]
mod tests {
    #[test]
    fn wsl_notice_cannot_hide_the_actual_git_failure() {
        let notice = "wsl: localhost proxy cannot be mirrored in NAT mode";
        let failure = "fatal: cannot access repository\nAuthentication failed";
        let message = super::command_error(notice, failure);
        assert!(message.contains(notice)); assert!(message.contains(failure));
        assert_eq!(super::command_error("", failure), failure);
    }

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

    /// A repository where `main` and `feature` changed the same line of `a.txt`.
    fn diverged() -> Option<(TempDir, Repo)> {
        let (d, r) = repo()?;
        r.run(&["config", "core.autocrlf", "false"]).ok()?;
        r.run(&["checkout", "-q", "-b", "main"]).ok()?;
        std::fs::write(d.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        r.commit("base", &["a.txt".into()]).ok()?;
        r.create_branch("feature").ok()?;
        std::fs::write(d.path().join("a.txt"), "one\nTWO from feature\nthree\n").unwrap();
        r.commit("feature", &["a.txt".into()]).ok()?;
        r.checkout("main").ok()?;
        std::fs::write(d.path().join("a.txt"), "one\ntwo from main\nthree\n").unwrap();
        r.commit("main", &["a.txt".into()]).ok()?;
        Some((d, r))
    }

    #[test]
    fn blame_history_stash_cherry_pick_revert_and_compare() {
        let Some((d, r)) = diverged() else { return };
        // blame: the line main changed is main's, the others the first commit's
        let blame = r.blame("a.txt").unwrap();
        assert_eq!(blame.len(), 3);
        assert_eq!(blame[1].summary, "main");
        assert_eq!(blame[0].summary, "base");
        assert!(blame[0].time > 0 && blame[0].author == "Tester" && !blame[0].uncommitted());
        // the history of the file, the newest first, and its content then
        let log = r.file_log("a.txt", 10);
        assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["main", "base"]);
        assert_eq!(r.file_at(&log[1].hash, "a.txt").as_deref(), Some("one\ntwo\nthree\n"));
        assert_eq!(r.path_at(&log[1].hash, "a.txt"), "a.txt");
        // what the other branch has that this one has not
        let (theirs, ours) = r.compare("feature", 50);
        assert_eq!((theirs.len(), ours.len(), theirs[0].subject.as_str()), (1, 1, "feature"));
        assert_eq!(r.compare_files("feature"), [("a.txt".to_string(), ChangeKind::Modified)]);
        // stash: put aside, listed, back
        std::fs::write(d.path().join("a.txt"), "one\ntwo from main\nthree\nfour\n").unwrap();
        std::fs::write(d.path().join("new.txt"), "new\n").unwrap();
        r.stash_save("work in progress", true).unwrap();
        assert!(!d.path().join("new.txt").exists());
        let stashes = r.stashes();
        assert_eq!((stashes.len(), stashes[0].name.as_str()), (1, "stash@{0}"));
        assert!(stashes[0].message.contains("work in progress"), "{:?}", stashes[0]);
        assert!(r.stash_save("", false).is_err(), "nothing to put aside");
        r.stash_do("pop", "stash@{0}").unwrap();
        assert!(d.path().join("new.txt").exists() && r.stashes().is_empty());
        r.stash_save("again", true).unwrap();
        r.stash_do("drop", "stash@{0}").unwrap();
        assert!(r.stashes().is_empty() && r.stash_do("rm", "stash@{0}").is_err());
        // cherry-pick of a commit that touches another file, then its revert
        r.checkout("feature").unwrap();
        std::fs::write(d.path().join("b.txt"), "b\n").unwrap();
        r.commit("add b", &["b.txt".into()]).unwrap();
        let b = r.head_hash().unwrap();
        r.checkout("main").unwrap();
        r.cherry_pick(&b).unwrap();
        assert_eq!(std::fs::read_to_string(d.path().join("b.txt")).unwrap(), "b\n");
        assert_eq!(r.log(1)[0].subject, "add b");
        r.revert(&r.head_hash().unwrap()).unwrap();
        assert!(!d.path().join("b.txt").exists());
        assert!(r.log(1)[0].subject.starts_with("Revert"));
        // cherry-picking the conflicting commit stops
        assert!(r.cherry_pick("feature~1").is_err());
        assert_eq!(r.merge_state(), Some(MergeKind::CherryPick));
        r.merge_abort(MergeKind::CherryPick).unwrap();
    }

    #[test]
    fn an_interactive_rebase_rewords_squashes_drops_and_reorders() {
        let Some((d, r)) = repo() else { return };
        r.run(&["config", "core.autocrlf", "false"]).unwrap();
        for (file, msg) in [("a", "first"), ("b", "second"), ("c", "third"), ("d", "fourth"), ("e", "fifth")] {
            std::fs::write(d.path().join(file), file).unwrap();
            r.commit(msg, &[file.to_string()]).unwrap();
        }
        let base = r.log(5)[4].hash.clone();
        let since = r.commits_since(&base);
        assert_eq!(since.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["second", "third", "fourth", "fifth"]);
        let step = |i: usize, action, message: &str| RebaseStep { hash: since[i].hash.clone(), action, message: message.into() };
        // fifth moves first and is reworded, third melts into second, fourth goes
        let steps = [step(3, RebaseAction::Reword, "fifth, reworded\n"), step(0, RebaseAction::Pick, ""), step(1, RebaseAction::Fixup, ""), step(2, RebaseAction::Drop, "")];
        r.rebase_interactive(&base, &steps).unwrap();
        assert_eq!(r.merge_state(), None);
        let subjects: Vec<String> = r.log(10).into_iter().map(|c| c.subject).collect();
        assert_eq!(subjects, ["second", "fifth, reworded", "first"]);
        assert!(!d.path().join("d").exists() && d.path().join("c").exists() && d.path().join("e").exists());
        assert!(r.rebase_interactive(&base, &[step(0, RebaseAction::Squash, "")]).is_err(), "nothing before it to melt into");
        let leftovers: Vec<_> = std::fs::read_dir(d.path().join(".git")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("light-rebase")).collect();
        assert!(leftovers.is_empty(), "the lists written for git are gone");
    }

    #[test]
    fn a_merge_with_conflicts_waits_and_is_continued_or_aborted() {
        let Some((d, r)) = diverged() else { return };
        assert_eq!(r.merge_state(), None);
        let err = r.merge("feature").unwrap_err();
        assert!(err.contains("a.txt"), "{err}");
        assert_eq!(r.merge_state(), Some(MergeKind::Merge));
        let text = std::fs::read_to_string(d.path().join("a.txt")).unwrap();
        let c = crate::conflicts::find(&text);
        assert_eq!((c.len(), c[0].ours.as_str(), c[0].theirs.as_str()), (1, "two from main\n", "TWO from feature\n"));
        assert_eq!(r.stage_content("a.txt", 3).as_deref(), Some("one\nTWO from feature\nthree\n"));
        assert_eq!(r.stage_content("a.txt", 1).as_deref(), Some("one\ntwo\nthree\n"));
        // abort: as before the merge
        r.merge_abort(MergeKind::Merge).unwrap();
        assert_eq!(r.merge_state(), None);
        assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "one\ntwo from main\nthree\n");
        // again, taking their side, and the merge is committed
        assert!(r.merge("feature").is_err());
        r.take_side("a.txt", false).unwrap();
        assert!(r.status().iter().all(|c| c.kind != ChangeKind::Conflicted));
        r.merge_continue(MergeKind::Merge).unwrap();
        assert_eq!(r.merge_state(), None);
        assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "one\nTWO from feature\nthree\n");
        assert_eq!(r.log(1)[0].parents.len(), 2, "a merge commit");
    }

    #[test]
    fn a_rebase_that_stops_for_conflicts_goes_on_once_resolved() {
        let Some((d, r)) = diverged() else { return };
        r.checkout("feature").unwrap();
        assert!(r.run(&["rebase", "main"]).is_err());
        assert_eq!(r.merge_state(), Some(MergeKind::Rebase));
        std::fs::write(d.path().join("a.txt"), "one\nboth\nthree\n").unwrap();
        r.stage(&["a.txt".into()]).unwrap();
        r.merge_continue(MergeKind::Rebase).unwrap();
        assert_eq!(r.merge_state(), None);
        assert_eq!(r.log(3).iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["feature", "main", "base"]);
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
    fn git_says_which_files_it_ignores_global_rules_included() {
        let Some((d, r)) = repo() else { return };
        let w = |name: &str, text: &str| std::fs::write(d.path().join(name), text).unwrap();
        w(".gitignore", "*.log\n");
        w("kept.log", "tracked by force\n");
        w("a.txt", "a\n");
        r.run(&["add", "-f", "kept.log", "a.txt", ".gitignore"]).unwrap();
        r.run(&["commit", "-q", "-m", "base"]).unwrap();
        w("debug.log", "x\n");
        w("new.txt", "x\n");
        w("scratch.tmp", "x\n");
        assert!(r.check_ignored("debug.log"));
        assert!(!r.check_ignored("new.txt"));
        assert!(!r.check_ignored("a.txt"));
        assert!(!r.check_ignored("kept.log"), "a tracked file is not ignored");
        assert!(!r.check_ignored("missing/nothing.txt"));
        // the user's own ignore file is not the project's, but git obeys it all the same
        assert!(!r.check_ignored("scratch.tmp"));
        let global = d.path().join("global-ignore");
        std::fs::write(&global, "*.tmp\n").unwrap();
        r.run(&["config", "core.excludesFile", &global.to_string_lossy().replace('\\', "/")]).unwrap();
        assert!(r.check_ignored("scratch.tmp"));
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

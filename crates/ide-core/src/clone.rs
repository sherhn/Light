//! "Get from Version Control": clone a repository (GitHub or any git URL) into a folder.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Turn what a user typed into a clonable URL.
///
/// * `owner/repo` -> `https://github.com/owner/repo.git`
/// * `github.com/owner/repo`, `https://github.com/owner/repo/` -> `https://github.com/owner/repo.git`
/// * `git@host:owner/repo.git`, `ssh://...`, `file://...` and local paths are passed through.
pub fn normalize_url(input: &str) -> Result<String, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("Enter a repository URL, e.g. https://github.com/owner/repo or owner/repo".into());
    }
    if s.contains(char::is_whitespace) {
        return Err("The URL must not contain spaces".into());
    }
    let is_scp = s.contains('@') && s.contains(':') && !s.contains("://");
    if s.starts_with("git@") || is_scp || s.starts_with("ssh://") || s.starts_with("file://") || s.starts_with("git://") {
        return Ok(s.to_string());
    }
    // local path (exists on disk, or drive/absolute/relative path)
    let looks_local = Path::new(s).is_absolute() || s.starts_with('.') || s.as_bytes().get(1) == Some(&b':');
    if looks_local {
        return Ok(s.to_string());
    }
    let mut u = s.to_string();
    if let Some(rest) = u.strip_prefix("http://") {
        u = format!("https://{rest}");
    }
    if !u.starts_with("https://") {
        let parts: Vec<&str> = u.trim_matches('/').split('/').collect();
        u = if parts.len() == 2 && !parts[0].contains('.') {
            format!("https://github.com/{}/{}", parts[0], parts[1])
        } else {
            format!("https://{}", u.trim_matches('/'))
        };
    }
    let u = u.trim_end_matches('/').to_string();
    let valid_host = u.strip_prefix("https://").is_some_and(|r| r.contains('/') && r.split('/').next().is_some_and(|h| h.contains('.')));
    if !valid_host {
        return Err(format!("'{input}' does not look like a repository URL"));
    }
    Ok(if u.ends_with(".git") { u } else { format!("{u}.git") })
}

/// Folder name for a repository URL: the last path segment without `.git`.
pub fn default_dir_name(url: &str) -> String {
    let last = url.trim_end_matches('/').rsplit(['/', ':', '\\']).next().unwrap_or("project");
    let name = last.strip_suffix(".git").unwrap_or(last);
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect();
    if cleaned.is_empty() { "project".into() } else { cleaned }
}

#[derive(Clone, Debug, Default)]
pub struct CloneOptions {
    /// The regular Linux account resolved by the IDE for a WSL destination.
    pub wsl_user: Option<String>,
    pub shallow: bool,
    pub branch: Option<String>,
    /// Extra environment for the git process (e.g. the credentials of a private GitHub repository).
    pub env: Vec<(String, String)>,
    /// Check files out with Linux line endings whatever Windows git is configured to do (for projects that are
    /// built in Linux: shell scripts and makefiles break on CRLF).
    pub lf: bool,
}

/// Percent of a git progress line (`Receiving objects:  42% (...)`), if it has one.
pub fn parse_progress(line: &str) -> Option<f32> {
    let i = line.find('%')?;
    let digits: String = line[..i].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse::<f32>().ok().map(|p| p.min(100.0))
}

/// Clone `url` into `dest` (which must not exist or be empty). `on_line` receives git's progress output
/// line by line; set `cancel` to abort. Returns the error text on failure.
pub fn clone_repo(url: &str, dest: &Path, opts: &CloneOptions, cancel: &Arc<AtomicBool>, on_line: &mut dyn FnMut(&str)) -> Result<(), String> {
    if dest.exists() && std::fs::read_dir(dest).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} already exists and is not empty", dest.display()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    // a folder on a WSL file system is cloned by the distribution's git, into its Linux path
    let wsl = crate::wsl::wsl_path(dest);
    let mut cmd = match &wsl {
        Some(w) => {
            let mut c = crate::wsl::command_as_user(&w.distro, opts.wsl_user.as_deref(), None, "git");
            let mut env = opts.env.clone();
            env.extend([("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()), ("LC_ALL".to_string(), "C".to_string())]);
            crate::wsl::pass_env(&mut c, &env);
            c
        }
        None => {
            let mut c = Command::new("git");
            c.envs(opts.env.iter().map(|(k, v)| (k, v)));
            c.env("GIT_TERMINAL_PROMPT", "0").env("LC_ALL", "C");
            c
        }
    };
    cmd.arg("clone").arg("--progress");
    if opts.lf && wsl.is_none() {
        cmd.args(["--config", "core.autocrlf=false", "--config", "core.eol=lf"]);
    }
    if opts.shallow {
        cmd.args(["--depth", "1"]);
    }
    if let Some(b) = opts.branch.as_deref().filter(|b| !b.trim().is_empty()) {
        cmd.args(["--branch", b.trim()]);
    }
    cmd.arg("--").arg(url);
    match &wsl {
        Some(w) => cmd.arg(&w.linux),
        None => cmd.arg(dest),
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot run git ({e}). Install git from https://git-scm.com"))?;
    let mut stderr = child.stderr.take().expect("piped");
    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    let mut tail: Vec<String> = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(dest);
            return Err("Cancelled".into());
        }
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                // git separates progress updates with \r and messages with \n
                while let Some(i) = pending.find(['\r', '\n']) {
                    let line: String = pending.drain(..=i).collect();
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        on_line(&line);
                        tail.push(line);
                        if tail.len() > 12 {
                            tail.remove(0);
                        }
                    }
                }
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_dir_all(dest);
        let msg = tail.iter().rev().find(|l| l.starts_with("fatal:") || l.starts_with("error:")).cloned().unwrap_or_else(|| tail.last().cloned().unwrap_or_else(|| "git clone failed".into()));
        Err(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;
    use crate::testutil::TempDir;

    #[test]
    fn url_normalisation() {
        assert_eq!(normalize_url("octocat/Hello-World").unwrap(), "https://github.com/octocat/Hello-World.git");
        assert_eq!(normalize_url("github.com/octocat/Hello-World").unwrap(), "https://github.com/octocat/Hello-World.git");
        assert_eq!(normalize_url("https://github.com/octocat/Hello-World/").unwrap(), "https://github.com/octocat/Hello-World.git");
        assert_eq!(normalize_url("http://gitlab.com/a/b.git").unwrap(), "https://gitlab.com/a/b.git");
        assert_eq!(normalize_url("  https://github.com/a/b  ").unwrap(), "https://github.com/a/b.git");
        assert_eq!(normalize_url("git@github.com:a/b.git").unwrap(), "git@github.com:a/b.git");
        assert_eq!(normalize_url("ssh://git@host/a/b").unwrap(), "ssh://git@host/a/b");
        assert_eq!(normalize_url("file:///tmp/x").unwrap(), "file:///tmp/x");
        assert_eq!(normalize_url(r"C:\repos\x").unwrap(), r"C:\repos\x");
        assert!(normalize_url("").is_err());
        assert!(normalize_url("has space/repo").is_err());
        assert!(normalize_url("justoneword").is_err());
    }

    #[test]
    fn folder_names() {
        assert_eq!(default_dir_name("https://github.com/octocat/Hello-World.git"), "Hello-World");
        assert_eq!(default_dir_name("git@github.com:a/b.git"), "b");
        assert_eq!(default_dir_name("https://host/a/b/"), "b");
        assert_eq!(default_dir_name(r"C:\repos\my repo"), "my-repo");
        assert_eq!(default_dir_name(""), "project");
    }

    #[test]
    fn progress_parsing() {
        assert_eq!(parse_progress("Receiving objects:  42% (21/50), 1.2 MiB | 2.0 MiB/s"), Some(42.0));
        assert_eq!(parse_progress("Resolving deltas: 100% (5/5), done."), Some(100.0));
        assert_eq!(parse_progress("Cloning into 'x'..."), None);
    }

    fn source_repo() -> Option<(TempDir, std::path::PathBuf)> {
        if !git::git_available() {
            return None;
        }
        let d = TempDir::new("clone-src");
        let src = d.path().join("upstream");
        std::fs::create_dir_all(&src).unwrap();
        let r = git::init(&src).ok()?;
        for (k, v) in [("user.email", "t@example.com"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            Command::new("git").args(["config", k, v]).current_dir(&src).output().ok()?;
        }
        std::fs::write(src.join("main.lisp"), "(defun hello () 1)\n").unwrap();
        r.commit("first", &["main.lisp".into()]).ok()?;
        Some((d, src))
    }

    #[test]
    fn clones_a_local_repository_with_progress() {
        let Some((d, src)) = source_repo() else { return };
        let dest = d.path().join("work").join("copy");
        let mut lines = Vec::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let url = normalize_url(&src.to_string_lossy()).unwrap();
        clone_repo(&url, &dest, &CloneOptions::default(), &cancel, &mut |l| lines.push(l.to_string())).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("main.lisp")).unwrap().replace("\r\n", "\n"), "(defun hello () 1)\n");
        assert!(dest.join(".git").exists());
        assert!(lines.iter().any(|l| l.contains("Cloning into")), "{lines:?}");
        // a second clone into the same non-empty folder is refused
        let err = clone_repo(&url, &dest, &CloneOptions::default(), &cancel, &mut |_| {}).unwrap_err();
        assert!(err.contains("not empty"), "{err}");
        assert!(dest.join("main.lisp").exists(), "an existing project must never be removed");
    }

    #[test]
    fn linux_line_endings_are_kept_on_request() {
        let Some((d, src)) = source_repo() else { return };
        std::fs::write(src.join("build.sh"), "#!/bin/sh\necho hi\n").unwrap();
        git::find_repo(&src).unwrap().commit("script", &["build.sh".into()]).unwrap();
        let url = normalize_url(&src.to_string_lossy()).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let dest = d.path().join("lf-copy");
        clone_repo(&url, &dest, &CloneOptions { lf: true, ..CloneOptions::default() }, &cancel, &mut |_| {}).unwrap();
        assert_eq!(std::fs::read(dest.join("build.sh")).unwrap(), b"#!/bin/sh\necho hi\n", "no CRLF in a file meant for Linux");
        let cfg = std::fs::read_to_string(dest.join(".git").join("config")).unwrap();
        assert!(cfg.contains("autocrlf = false"), "{cfg}");
    }

    #[test]
    fn failed_clone_reports_git_error_and_cleans_up() {
        if !git::git_available() {
            return;
        }
        let d = TempDir::new("clone-bad");
        let dest = d.path().join("out");
        let cancel = Arc::new(AtomicBool::new(false));
        let err = clone_repo(&d.path().join("no-such-repo").to_string_lossy(), &dest, &CloneOptions::default(), &cancel, &mut |_| {}).unwrap_err();
        assert!(err.to_lowercase().contains("fatal") || err.to_lowercase().contains("error") || err.contains("does not exist"), "{err}");
        assert!(!dest.exists());
    }

    #[test]
    fn branch_and_cancel() {
        let Some((d, src)) = source_repo() else { return };
        let repo = git::find_repo(&src).unwrap();
        repo.create_branch("dev").unwrap();
        std::fs::write(src.join("dev.lisp"), "(dev)\n").unwrap();
        repo.commit("dev work", &["dev.lisp".into()]).unwrap();
        let url = normalize_url(&src.to_string_lossy()).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let dest = d.path().join("dev-copy");
        clone_repo(&url, &dest, &CloneOptions { branch: Some("dev".into()), ..CloneOptions::default() }, &cancel, &mut |_| {}).unwrap();
        assert!(dest.join("dev.lisp").exists());
        // cancelled before it starts reading: no leftovers
        let cancel2 = Arc::new(AtomicBool::new(true));
        let dest2 = d.path().join("cancelled");
        let r = clone_repo(&url, &dest2, &CloneOptions::default(), &cancel2, &mut |_| {});
        assert!(r.is_err());
        assert!(!dest2.exists());
    }
}

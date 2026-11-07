//! The PATH of the user's login shell. Light started from the macOS Finder or Dock, or from a Linux desktop menu, gets
//! the short PATH of the desktop (`/usr/bin:/bin:/usr/sbin:/sbin` on macOS) without Homebrew, npm or the user's own
//! folders, so the run commands, the checks and the language servers would not find what the terminal finds. Light
//! started that way asks the login shell once, at start, and puts its folders first.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const START: &str = "__LIGHT_ENV_START__";
const END: &str = "__LIGHT_ENV_END__";

/// Was Light started from a terminal, whose PATH is the user's already?
pub fn from_terminal() -> bool {
    std::env::var_os("TERM").is_some_and(|t| !t.is_empty())
}

/// The shell to ask: the user's (`SHELL`), else the system's.
pub fn user_shell() -> String {
    match std::env::var("SHELL") {
        Ok(s) if !s.trim().is_empty() => s,
        _ if cfg!(target_os = "macos") => "/bin/zsh".into(),
        _ => "/bin/sh".into(),
    }
}

/// The value of PATH in what `env` printed between the markers; whatever the profile printed around it is left out.
fn path_in(out: &str) -> Option<String> {
    let mut lines = out.lines().skip_while(|l| l.trim() != START).skip(1);
    lines.by_ref().take_while(|l| l.trim() != END).find_map(|l| l.strip_prefix("PATH=")).map(str::to_string).filter(|p| !p.is_empty())
}

/// The PATH the login shell `shell` sets up, asked as an interactive login shell (the profile and the interactive
/// setup, where tools such as nvm put themselves). `None` when it does not answer within `timeout`; a shell that
/// waits for something is ended.
pub fn login_path(shell: &str, timeout: Duration) -> Option<String> {
    let script = format!("echo {START}; env; echo {END}");
    let mut child = Command::new(shell).args(["-l", "-i", "-c", &script]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    // a process the profile started in the background may keep the output open after the shell is gone: the reader
    // is not waited for longer than the shell
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = rx.recv_timeout(Duration::from_millis(500)).ok()?;
    path_in(&String::from_utf8_lossy(&out))
}

/// PATH made of the login shell's folders, then those of `current` it does not have.
pub fn merge(login: &str, current: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for p in login.split(':').chain(current.split(':')) {
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    }
    out.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_is_taken_from_between_the_markers() {
        let out = format!("Welcome!\nPATH=/not/this\n{START}\nHOME=/home/me\nPATH=/opt/homebrew/bin:/usr/bin\nX=1\n{END}\nbye\n");
        assert_eq!(path_in(&out).as_deref(), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(path_in("PATH=/usr/bin\n"), None, "no markers: nothing trusted");
        assert_eq!(path_in(&format!("{START}\nPATH=\n{END}\n")), None);
    }

    #[test]
    fn the_login_folders_come_first_without_repeats() {
        assert_eq!(merge("/opt/homebrew/bin:/usr/bin", "/usr/bin:/bin:/usr/sbin"), "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin");
        assert_eq!(merge("", "/usr/bin::/bin"), "/usr/bin:/bin");
    }

    #[cfg(unix)]
    #[test]
    fn a_login_shell_answers() {
        let path = login_path("/bin/sh", Duration::from_secs(20)).expect("sh answers");
        assert!(path.split(':').any(|p| p == "/usr/bin" || p == "/bin"), "{path}");
        assert!(login_path("/definitely/not/a/shell", Duration::from_secs(1)).is_none());
        // a shell whose profile waits is ended when the time is up
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testutil::TempDir::new("slow-shell");
        let slow = dir.path().join("slow-shell");
        std::fs::write(&slow, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
        let start = Instant::now();
        assert!(login_path(slow.to_str().unwrap(), Duration::from_secs(1)).is_none());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}

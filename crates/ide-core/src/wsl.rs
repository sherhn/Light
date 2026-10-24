//! Folders on a WSL distribution's own file system, which Windows sees as `\\wsl.localhost\<distro>\...`.
//!
//! A project that lives there is a Linux project: git for it runs inside the distribution (so file modes, symlinks
//! and line endings stay Linux ones and git does not complain about "dubious ownership"). Every call goes through
//! `wsl.exe`; in the UI tests `LIGHT_FAKE_WSL` (a Python script) stands in for it.

use std::path::Path;
use std::process::Command;

/// A folder inside a distribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslPath {
    pub distro: String,
    /// The path as Linux programs see it (`/home/me/os-pi`).
    pub linux: String,
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)).then(|| &s[prefix.len()..])
}

/// `\\wsl.localhost\Ubuntu\home\me` (also `\\wsl$\...` and the `\\?\UNC\...` form) -> Ubuntu, `/home/me`.
pub fn wsl_path(p: &Path) -> Option<WslPath> {
    let raw = p.to_string_lossy().replace('\\', "/");
    let s = strip_prefix_ci(&raw, "//?/UNC/").map(|r| format!("//{r}")).unwrap_or(raw);
    for host in ["//wsl.localhost/", "//wsl$/"] {
        if let Some(rest) = strip_prefix_ci(&s, host) {
            let (distro, inside) = rest.split_once('/').unwrap_or((rest, ""));
            if distro.is_empty() {
                return None;
            }
            let inside = inside.trim_end_matches('/');
            return Some(WslPath { distro: distro.to_string(), linux: format!("/{inside}") });
        }
    }
    None
}

/// The Windows path of `linux` inside `distro`: `/home/me` -> `\\wsl.localhost\<distro>\home\me`.
pub fn windows_path(distro: &str, linux: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("\\\\wsl.localhost\\{distro}{}", linux.replace('/', "\\")))
}

/// `wsl.exe -d DISTRO [--cd DIR] -e PROGRAM` (arguments are added by the caller). In the UI tests
/// `LIGHT_FAKE_WSL` (a Python script, run by `LIGHT_FAKE_WSL_PYTHON`) replaces `wsl.exe`.
pub fn command(distro: &str, cwd: Option<&str>, program: &str) -> Command {
    let mut c = match std::env::var_os("LIGHT_FAKE_WSL").or_else(|| std::env::var_os("AIR_FAKE_WSL")) {
        Some(script) => {
            let mut c = Command::new(std::env::var_os("LIGHT_FAKE_WSL_PYTHON").or_else(|| std::env::var_os("AIR_FAKE_WSL_PYTHON")).unwrap_or_else(|| "python".into()));
            c.arg(script);
            c
        }
        None => Command::new("wsl.exe"),
    };
    c.env("WSL_UTF8", "1");
    c.args(["-d", distro]);
    if let Some(dir) = cwd {
        c.args(["--cd", dir]);
    }
    c.args(["-e", program]);
    c
}

/// Give `cmd` (a `wsl.exe` call) environment variables that reach the Linux program too: WSL passes on the ones
/// listed in `WSLENV`. Values never appear on a command line, so tokens stay out of process listings.
pub fn pass_env(cmd: &mut Command, vars: &[(String, String)]) {
    if vars.is_empty() {
        return;
    }
    let mut names: Vec<String> = std::env::var("WSLENV").ok().filter(|v| !v.is_empty()).map(|v| v.split(':').map(str::to_string).collect()).unwrap_or_default();
    for (k, v) in vars {
        cmd.env(k, v);
        if !names.iter().any(|n| n == k) {
            names.push(k.clone());
        }
    }
    cmd.env("WSLENV", names.join(":"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(c: &Command) -> Vec<String> {
        c.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn linux_folders_are_recognised() {
        let w = wsl_path(Path::new(r"\\wsl.localhost\Ubuntu-24.04\home\me\os-pi")).unwrap();
        assert_eq!(w, WslPath { distro: "Ubuntu-24.04".into(), linux: "/home/me/os-pi".into() });
        assert_eq!(wsl_path(Path::new(r"\\wsl$\Debian\srv\")).unwrap().linux, "/srv");
        assert_eq!(wsl_path(Path::new(r"\\?\UNC\wsl.localhost\Ubuntu\home")).unwrap(), WslPath { distro: "Ubuntu".into(), linux: "/home".into() });
        assert_eq!(wsl_path(Path::new(r"\\WSL.LOCALHOST\Ubuntu")).unwrap().linux, "/");
        assert_eq!(wsl_path(Path::new(r"C:\Users\me\proj")), None);
        assert_eq!(wsl_path(Path::new(r"\\server\share\x")), None);
        assert_eq!(windows_path("Ubuntu", "/home/me/x"), std::path::PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\x"));
    }

    #[test]
    fn commands_go_through_wsl_exe() {
        if std::env::var_os("LIGHT_FAKE_WSL").or_else(|| std::env::var_os("AIR_FAKE_WSL")).is_some() {
            return;
        }
        let mut c = command("Ubuntu", Some("/home/me/p"), "git");
        c.arg("status");
        assert_eq!(c.get_program().to_string_lossy(), "wsl.exe");
        assert_eq!(args(&c), ["-d", "Ubuntu", "--cd", "/home/me/p", "-e", "git", "status"]);
        pass_env(&mut c, &[("GIT_CONFIG_COUNT".into(), "1".into()), ("LC_ALL".into(), "C".into())]);
        let wslenv = c.get_envs().find(|(k, _)| *k == "WSLENV").and_then(|(_, v)| v).unwrap().to_string_lossy().into_owned();
        assert!(wslenv.split(':').any(|n| n == "GIT_CONFIG_COUNT") && wslenv.split(':').any(|n| n == "LC_ALL"));
        // the secret itself is an environment variable, never an argument
        assert!(!args(&c).iter().any(|a| a.contains("GIT_CONFIG")));
    }
}

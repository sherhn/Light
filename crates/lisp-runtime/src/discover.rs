//! Locating a Lisp implementation on this machine.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::wsl::{self, Cleanup, Target};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Implementation {
    pub name: String,
    /// The executable: a Windows path, or (with `wsl`) a command or path inside the distribution.
    pub path: PathBuf,
    /// `SBCL_HOME` for the bundled distribution (core and contribs live next to the executable).
    pub home: Option<PathBuf>,
    /// Run inside this WSL distribution instead of on Windows.
    pub wsl: Option<Target>,
}

/// An argument of a Lisp process: plain text, or a file whose path has to be given the way Lisp sees files.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    Text(&'a str),
    Path(&'a Path),
}

/// A command ready to spawn, plus how to stop what it starts inside WSL.
pub struct Launch {
    pub cmd: Command,
    pub cleanup: Option<Cleanup>,
}

impl Implementation {
    pub fn sbcl(path: impl Into<PathBuf>) -> Implementation {
        Implementation { name: "SBCL".into(), path: path.into(), home: None, wsl: None }
    }

    /// An SBCL shipped inside `dir` (`dir/sbcl[.exe]`, `dir/sbcl.core`, `dir/contrib`).
    pub fn bundled(dir: &Path) -> Option<Implementation> {
        let exe = dir.join(exe_name());
        (is_file(&exe) && dir.join("sbcl.core").is_file()).then(|| Implementation { name: "SBCL".into(), path: exe, home: Some(dir.to_path_buf()), wsl: None })
    }

    /// SBCL inside a WSL distribution; `sbcl` is its command or absolute path there (empty: `sbcl`).
    pub fn in_wsl(target: Target, sbcl: &str) -> Implementation {
        let sbcl = if sbcl.trim().is_empty() { "sbcl" } else { sbcl.trim() };
        Implementation { name: format!("SBCL ({})", target.distro), path: PathBuf::from(sbcl), home: None, wsl: Some(target) }
    }

    /// Which binary and where, for messages: `C:\...\sbcl.exe` or `/usr/bin/sbcl (WSL: Ubuntu-24.04)`.
    pub fn describe(&self) -> String {
        match &self.wsl {
            Some(t) => format!("{} (WSL: {})", self.path.to_string_lossy(), t.distro),
            None => self.path.to_string_lossy().into_owned(),
        }
    }

    /// `p` the way this Lisp names files, with forward slashes (`C:/x/a.lisp`, or `/mnt/c/x/a.lisp` in WSL).
    pub fn lisp_path(&self, p: &Path) -> String {
        match &self.wsl {
            Some(t) => wsl::to_linux(p, t),
            None => p.to_string_lossy().replace('\\', "/"),
        }
    }

    /// The Windows path of a file name this Lisp reported (e.g. a definition's source file).
    pub fn host_path(&self, name: &str) -> PathBuf {
        match &self.wsl {
            Some(t) if name.starts_with('/') => wsl::to_windows(name, t),
            _ => PathBuf::from(name),
        }
    }

    /// The command for this Lisp with `args`, working in `cwd`. Inside WSL the paths are translated and the
    /// process gets a tag so it can be ended even if stopping `wsl.exe` leaves it running.
    pub fn launch(&self, cwd: Option<&Path>, args: &[Arg]) -> Launch {
        match &self.wsl {
            None => {
                let mut cmd = Command::new(&self.path);
                apply_env(&mut cmd, self);
                for a in args {
                    match a {
                        Arg::Text(s) => cmd.arg(s),
                        Arg::Path(p) => cmd.arg(p),
                    };
                }
                if let Some(dir) = cwd {
                    cmd.current_dir(dir);
                }
                Launch { cmd, cleanup: None }
            }
            Some(t) => {
                let marker = wsl::new_marker();
                let mut list: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        Arg::Text(s) => s.to_string(),
                        Arg::Path(p) => wsl::to_linux(p, t),
                    })
                    .collect();
                // SBCL ignores what follows; the tag only has to be on the command line
                list.extend(["--end-toplevel-options".to_string(), marker.clone()]);
                let cmd = wsl::exec(t, cwd, &self.path.to_string_lossy(), &list);
                Launch { cmd, cleanup: Some(Cleanup { target: t.clone(), marker }) }
            }
        }
    }

    /// Query the version string, e.g. `SBCL 2.6.8`.
    pub fn version(&self) -> Option<String> {
        let Launch { mut cmd, .. } = self.launch(None, &[Arg::Text("--version")]);
        crate::no_window(&mut cmd);
        let out = cmd.output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "sbcl.exe" } else { "sbcl" }
}

fn is_file(p: &Path) -> bool {
    p.is_file()
}

/// Candidate locations in priority order. `configured` is the user's explicit setting.
pub fn candidates(configured: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if !configured.trim().is_empty() {
        v.push(PathBuf::from(configured.trim()));
    }
    if let Ok(p) = std::env::var("LIGHT_SBCL") {
        v.push(PathBuf::from(p));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            v.push(dir.join(exe_name()));
        }
    }
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(var) {
                v.push(Path::new(&base).join("Steel Bank Common Lisp").join("sbcl.exe"));
                v.push(Path::new(&base).join("Programs").join("Steel Bank Common Lisp").join("sbcl.exe"));
            }
        }
    } else {
        for p in ["/usr/bin/sbcl", "/usr/local/bin/sbcl", "/opt/homebrew/bin/sbcl", "/opt/local/bin/sbcl", "/snap/bin/sbcl"] {
            v.push(PathBuf::from(p));
        }
        if let Some(home) = std::env::var_os("HOME") {
            v.push(Path::new(&home).join(".roswell/bin/sbcl"));
            v.push(Path::new(&home).join(".guix-profile/bin/sbcl"));
            v.push(Path::new(&home).join(".nix-profile/bin/sbcl"));
        }
    }
    v
}

/// Directory of the SBCL that ships with Light: `LIGHT_BUNDLE_DIR`, else the installed bundle.
pub fn bundle_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("LIGHT_BUNDLE_DIR") {
        return Some(PathBuf::from(d));
    }
    runtime_dir_for(&std::env::current_exe().ok()?)
}

fn runtime_dir_for(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    #[cfg(target_os = "macos")]
    if directory.file_name().is_some_and(|name| name == "MacOS")
        && directory.parent()?.file_name().is_some_and(|name| name == "Contents")
    {
        return Some(directory.parent()?.join("Resources/lisp"));
    }
    Some(directory.join("lisp"))
}

/// Find SBCL: the explicit setting, `LIGHT_SBCL`, the bundled one, `PATH`, then well-known install locations.
pub fn find_sbcl(configured: &str) -> Option<Implementation> {
    let mut explicit = Vec::new();
    if !configured.trim().is_empty() {
        explicit.push(PathBuf::from(configured.trim()));
    }
    if let Ok(p) = std::env::var("LIGHT_SBCL") {
        explicit.push(PathBuf::from(p));
    }
    if let Some(p) = explicit.into_iter().find(|p| is_file(p)) {
        return Some(Implementation::sbcl(p));
    }
    if let Some(b) = bundle_dir().and_then(|d| Implementation::bundled(&d)) {
        return Some(b);
    }
    candidates("").into_iter().find(|p| is_file(p)).map(Implementation::sbcl)
}

/// Environment every SBCL process needs (applies `SBCL_HOME` for the bundled distribution).
pub fn apply_env(cmd: &mut Command, imp: &Implementation) {
    if let Some(h) = &imp.home {
        cmd.env("SBCL_HOME", h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_is_next_to_portable_executables() {
        assert_eq!(runtime_dir_for(Path::new("portable/light")), Some(PathBuf::from("portable/lisp")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_bundles_keep_runtime_data_in_resources() {
        for executable in ["Light.app/Contents/MacOS/light", "Light.app/Contents/MacOS/light-cli"] {
            assert_eq!(runtime_dir_for(Path::new(executable)), Some(PathBuf::from("Light.app/Contents/Resources/lisp")));
        }
    }

    #[test]
    fn wsl_implementations_translate_paths_and_tag_the_process() {
        let imp = Implementation::in_wsl(Target::new("Ubuntu-24.04", ""), "");
        assert_eq!(imp.path, PathBuf::from("sbcl"));
        assert_eq!(imp.lisp_path(Path::new(r"C:\p\a.lisp")), "/mnt/c/p/a.lisp");
        assert_eq!(imp.host_path("/mnt/c/p/a.lisp"), PathBuf::from(r"C:\p\a.lisp"));
        assert!(imp.describe().contains("Ubuntu-24.04"));
        let l = imp.launch(Some(Path::new(r"C:\p")), &[Arg::Text("--load"), Arg::Path(Path::new(r"C:\p\a.lisp"))]);
        let args: Vec<String> = l.cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let marker = l.cleanup.as_ref().unwrap().marker.clone();
        let at = args.iter().position(|a| a == "-d").unwrap();
        assert_eq!(args[at..], ["-d", "Ubuntu-24.04", "--cd", "/mnt/c/p", "-e", "sbcl", "--load", "/mnt/c/p/a.lisp", "--end-toplevel-options", marker.as_str()]);
        let native = Implementation::sbcl(r"C:\sbcl\sbcl.exe");
        assert_eq!(native.lisp_path(Path::new(r"C:\p\a.lisp")), "C:/p/a.lisp");
        assert_eq!(native.host_path("C:/p/a.lisp"), PathBuf::from("C:/p/a.lisp"));
        let l = native.launch(None, &[Arg::Path(Path::new(r"C:\p\a.lisp"))]);
        assert!(l.cleanup.is_none());
        assert_eq!(l.cmd.get_program(), std::ffi::OsStr::new(r"C:\sbcl\sbcl.exe"));
    }

    #[test]
    fn bundled_requires_executable_and_core() {
        let dir = std::env::temp_dir().join(format!("light-bundle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Implementation::bundled(&dir).is_none());
        std::fs::write(dir.join(exe_name()), b"x").unwrap();
        assert!(Implementation::bundled(&dir).is_none(), "the core is required too");
        std::fs::write(dir.join("sbcl.core"), b"x").unwrap();
        let b = Implementation::bundled(&dir).unwrap();
        assert_eq!(b.home.as_deref(), Some(dir.as_path()));
        assert_eq!(b.path, dir.join(exe_name()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configured_path_has_priority() {
        let c = candidates("C:/custom/sbcl.exe");
        assert_eq!(c[0], PathBuf::from("C:/custom/sbcl.exe"));
        assert!(candidates("").len() > 1);
    }

    #[test]
    fn missing_configured_path_falls_through() {
        // a bogus explicit path must not hide a working system install
        let found = find_sbcl("Z:/definitely/not/here/sbcl.exe");
        if let Some(f) = found {
            assert!(f.path.is_file());
        }
    }

    #[test]
    fn finds_sbcl_and_reports_version_when_installed() {
        if let Some(i) = find_sbcl("") {
            let v = i.version().expect("version output");
            assert!(v.to_lowercase().contains("sbcl"), "{v}");
        }
    }
}

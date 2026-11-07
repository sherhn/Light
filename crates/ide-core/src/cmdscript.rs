//! Command scripts for the terminal (and `light-cli script`): a text file with one shell command per line,
//! run one after another in the same working directory.
//!
//! ```text
//! # comment (also //, :: and REM)
//! cd src
//! set GREETING=hello
//! echo %GREETING%            <- plain commands go to the system shell
//! -git status                <- a leading '-' ignores a failing exit code
//! @echo quiet                <- a leading '@' does not echo the command line
//! exit 0                     <- stops the script (optional code)
//! ```
//!
//! `cd`, `set`/`export`, `clear` and `exit` are handled by the frontend itself (see [`builtin`]) so that the working
//! directory and variables persist between lines; everything else is handed to `cmd /C` or `sh -c`.
//! Files with a native extension (`.bat`, `.cmd`, `.ps1`, `.sh`) are not parsed but executed by their interpreter.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One command of a script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// 1-based line number in the script file.
    pub line: usize,
    pub text: String,
    /// `@` prefix: run without echoing the command line.
    pub silent: bool,
    /// `-` prefix: a non-zero exit code does not stop the script.
    pub ignore_errors: bool,
}

/// Extensions that are executed by their own interpreter instead of being parsed as command scripts.
pub const NATIVE_EXTENSIONS: &[&str] = &["bat", "cmd", "ps1", "sh"];
/// The extension of Light command scripts.
pub const SCRIPT_EXTENSION: &str = "als";

/// Is `path` something that "Run script" should offer (native scripts and `.als` files)?
pub fn is_script_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        let e = e.to_ascii_lowercase();
        e == SCRIPT_EXTENSION || NATIVE_EXTENSIONS.contains(&e.as_str())
    })
}

fn is_comment(line: &str) -> bool {
    let l = line.trim_start();
    l.starts_with('#') || l.starts_with("//") || l.starts_with("::") || l.eq_ignore_ascii_case("rem") || l.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("rem ") || p.eq_ignore_ascii_case("rem\t"))
}

/// Split a script into steps. Blank lines and comments are skipped; a trailing ` \` or ` ^` joins a line with the next.
pub fn parse(text: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut pending: Option<(usize, String)> = None;
    for (i, raw) in text.lines().enumerate() {
        let raw = raw.trim_end_matches('\r');
        let (start, mut acc) = match pending.take() {
            Some((start, mut acc)) => {
                acc.push(' ');
                acc.push_str(raw.trim());
                (start, acc)
            }
            None => {
                if raw.trim().is_empty() || is_comment(raw) {
                    continue;
                }
                (i + 1, raw.trim().to_string())
            }
        };
        let trimmed = acc.trim_end();
        if (trimmed.ends_with(" \\") || trimmed.ends_with(" ^")) && trimmed.len() > 2 {
            acc = trimmed[..trimmed.len() - 1].trim_end().to_string();
            pending = Some((start, acc));
            continue;
        }
        let (mut silent, mut ignore_errors) = (false, false);
        let mut body = acc.as_str();
        loop {
            if let Some(r) = body.strip_prefix('@') {
                silent = true;
                body = r.trim_start();
            } else if let Some(r) = body.strip_prefix('-') {
                ignore_errors = true;
                body = r.trim_start();
            } else {
                break;
            }
        }
        if !body.is_empty() {
            steps.push(Step { line: start, text: body.to_string(), silent, ignore_errors });
        }
    }
    if let Some((start, acc)) = pending {
        steps.push(Step { line: start, text: acc, silent: false, ignore_errors: false });
    }
    steps
}

/// Commands the frontend executes itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Builtin {
    Clear,
    /// `cd <dir>` (the argument may be empty = home).
    Cd(String),
    /// `set NAME=value` / `export NAME=value`.
    Set(String, String),
    /// `exit [code]`: ends a script.
    Exit(i32),
}

/// Recognise a built-in command. Returns `None` for everything that belongs to the system shell
/// (including a bare `set`, which lists the environment).
pub fn builtin(line: &str) -> Option<Builtin> {
    let t = line.trim();
    let lower = t.to_lowercase();
    if lower == "clear" || lower == "cls" {
        return Some(Builtin::Clear);
    }
    if lower == "cd" || lower.starts_with("cd ") || lower.starts_with("cd\t") {
        return Some(Builtin::Cd(t[2..].trim().to_string()));
    }
    if lower == "exit" || lower.starts_with("exit ") {
        return Some(Builtin::Exit(t[4..].trim().parse().unwrap_or(0)));
    }
    for kw in ["set ", "export "] {
        if lower.starts_with(kw) {
            let rest = t[kw.len()..].trim();
            let rest = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')).unwrap_or(rest);
            if let Some((name, value)) = rest.split_once('=') {
                let name = name.trim();
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    let value = value.trim();
                    let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
                    return Some(Builtin::Set(name.to_string(), value.to_string()));
                }
            }
        }
    }
    None
}

/// Resolve `cd <arg>` relative to `cwd`.
pub fn change_dir(cwd: &Path, arg: &str) -> Result<PathBuf, String> {
    let arg = arg.trim().trim_matches('"');
    let target = if arg.is_empty() || arg == "~" {
        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).ok_or("no home directory")?
    } else {
        let p = Path::new(arg);
        if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }
    };
    let t = target.canonicalize().map_err(|_| format!("The system cannot find the path specified: {arg}"))?;
    if !t.is_dir() {
        return Err(format!("Not a directory: {arg}"));
    }
    // avoid the verbatim `\\?\` prefix on Windows
    let s = t.to_string_lossy();
    Ok(PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()))
}

/// A command line in the platform shell (`cmd /C` or `sh -c`).
pub fn shell_command(line: &str, cwd: &Path, env: &[(String, String)]) -> Command {
    #[cfg(windows)]
    let mut c = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.args(["/D", "/Q", "/C"]).raw_arg(format!("\"{line}\""));
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = Command::new("sh");
        c.args(["-c", line]);
        c
    };
    c.current_dir(cwd);
    c.envs(env.iter().map(|(k, v)| (k, v)));
    c
}

/// The interpreter command for a native script file (`.bat`, `.cmd`, `.ps1`, `.sh`), run in `cwd`.
pub fn native_command(path: &Path, cwd: &Path, env: &[(String, String)]) -> Option<Command> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let mut c = match ext.as_str() {
        "bat" | "cmd" => {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let mut c = Command::new("cmd");
                c.args(["/D", "/Q", "/C"]).raw_arg(format!("\"call \"{}\"\"", path.display()));
                c
            }
            #[cfg(not(windows))]
            {
                return None;
            }
        }
        "ps1" => {
            let mut c = Command::new(if cfg!(windows) { "powershell" } else { "pwsh" });
            c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(path);
            c
        }
        "sh" => {
            let mut c = Command::new("sh");
            c.arg(path);
            c
        }
        _ => return None,
    };
    c.current_dir(cwd);
    c.envs(env.iter().map(|(k, v)| (k, v)));
    Some(c)
}

/// Where the shell would find `program` in the folders of `path` (a PATH value): the file itself, or on Windows the
/// first with an extension of `PATHEXT` (`clangd` is `clangd.exe`, `npm` is `npm.cmd`). A name with a folder in it is
/// looked at as it is.
pub fn find_program(program: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        // `cmd` runs a file without an extension of PATHEXT only when it was named with one
        let named = Path::new(program).extension().map(|_| String::new());
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        named.into_iter().chain(pathext.split(';').filter(|e| !e.is_empty()).map(str::to_lowercase)).collect()
    } else {
        vec![String::new()]
    };
    let usable = |p: &Path| -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        {
            p.is_file()
        }
    };
    let candidates = |base: &Path| -> Option<PathBuf> { exts.iter().map(|e| PathBuf::from(format!("{}{e}", base.display()))).find(|p| usable(p)) };
    if program.contains(['/', '\\']) {
        return candidates(Path::new(program));
    }
    std::env::split_paths(path?).filter(|d| !d.as_os_str().is_empty()).find_map(|d| candidates(&d.join(program)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_are_found_on_the_path() {
        let dir = crate::testutil::TempDir::new("find-program");
        let name = if cfg!(windows) { "tool.cmd" } else { "tool" };
        let tool = dir.path().join(name);
        std::fs::write(&tool, "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([Path::new("/definitely/not/here"), dir.path()]).unwrap();
        assert_eq!(find_program("tool", Some(&path)), Some(tool.clone()));
        assert_eq!(find_program("no-such-tool", Some(&path)), None);
        assert_eq!(find_program("tool", None), None);
        // with a folder: that file
        assert_eq!(find_program(&dir.path().join("tool").to_string_lossy(), None), Some(tool));
        // the system's own shell is found on the real PATH
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(find_program(shell, std::env::var_os("PATH").as_deref()).is_some());
    }

    #[test]
    fn parses_comments_prefixes_and_continuations() {
        let text = "# build\n\n  cd src  \n// note\n:: note\nREM also a note\nrem\n-git status\n@echo quiet\n@-echo both\necho a \\\n  b\nlast";
        let steps = parse(text);
        let texts: Vec<_> = steps.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["cd src", "git status", "echo quiet", "echo both", "echo a b", "last"]);
        assert_eq!(steps[0].line, 3);
        assert!(steps[1].ignore_errors && !steps[1].silent);
        assert!(steps[2].silent && !steps[2].ignore_errors);
        assert!(steps[3].silent && steps[3].ignore_errors);
        assert_eq!(steps[4].line, 11, "a joined command keeps the line of its first part");
    }

    #[test]
    fn crlf_and_empty_scripts() {
        assert!(parse("").is_empty());
        assert!(parse("\r\n# x\r\n   \r\n").is_empty());
        assert_eq!(parse("echo 1\r\necho 2\r\n").len(), 2);
        // a Windows path ending in a backslash is not a continuation
        assert_eq!(parse("cd C:\\work\\\necho x").len(), 2);
    }

    #[test]
    fn recognises_builtins() {
        assert_eq!(builtin("cls"), Some(Builtin::Clear));
        assert_eq!(builtin("CLEAR"), Some(Builtin::Clear));
        assert_eq!(builtin("cd"), Some(Builtin::Cd(String::new())));
        assert_eq!(builtin("cd src/sub"), Some(Builtin::Cd("src/sub".into())));
        assert_eq!(builtin("cdx"), None);
        assert_eq!(builtin("exit"), Some(Builtin::Exit(0)));
        assert_eq!(builtin("exit 3"), Some(Builtin::Exit(3)));
        assert_eq!(builtin("set A=1"), Some(Builtin::Set("A".into(), "1".into())));
        assert_eq!(builtin("export NAME=\"x y\""), Some(Builtin::Set("NAME".into(), "x y".into())));
        assert_eq!(builtin("set \"B=2\""), Some(Builtin::Set("B".into(), "2".into())));
        assert_eq!(builtin("set"), None, "a bare set lists the environment: shell business");
        assert_eq!(builtin("set /a 1+1"), None);
        assert_eq!(builtin("echo hi"), None);
    }

    #[test]
    fn script_file_detection() {
        for f in ["a.bat", "a.CMD", "x/b.ps1", "c.sh", "d.als"] {
            assert!(is_script_file(Path::new(f)), "{f}");
        }
        for f in ["a.lisp", "a.txt", "als", "a"] {
            assert!(!is_script_file(Path::new(f)), "{f}");
        }
    }

    #[test]
    fn cd_resolves_relative_and_rejects_missing() {
        let base = std::env::temp_dir().join(format!("light-cmdscript-{}", std::process::id()));
        std::fs::create_dir_all(base.join("sub")).unwrap();
        let base = change_dir(&base, ".").unwrap();
        let sub = change_dir(&base, "sub").unwrap();
        assert!(sub.ends_with("sub"));
        assert_eq!(change_dir(&sub, "..").unwrap(), base);
        assert!(change_dir(&base, "does-not-exist").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn shell_runs_commands_with_env() {
        let env = vec![("LIGHT_T".to_string(), "value-42".to_string())];
        let line = if cfg!(windows) { "echo %LIGHT_T%" } else { "echo $LIGHT_T" };
        let out = shell_command(line, Path::new("."), &env).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("value-42"), "{:?}", String::from_utf8_lossy(&out.stdout));
        let fail = if cfg!(windows) { "exit /b 3" } else { "exit 3" };
        assert_eq!(shell_command(fail, Path::new("."), &[]).output().unwrap().status.code(), Some(3));
    }
}

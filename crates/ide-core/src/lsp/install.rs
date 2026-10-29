//! Installing a language server the way its module says ([`InstallDef`], `docs/language-servers.md`): the folder it
//! goes to, the place it runs (this computer or a WSL distribution) and the steps - download and check, unpack,
//! `npm ci` - one after another. A unit (the runtime, the server) is made in `<folder>.part` and moved into place with
//! its stamp last, so a broken install is never taken for a whole one; an installed unit is left as it is.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::modules::{ArchiveDef, LspDef};
use crate::modules::sha256::sha256_hex;

/// The file that says a unit is installed; it is written last.
pub const STAMP: &str = ".light-installed";

/// The platform of this computer (one of [`crate::modules::PLATFORMS`]).
pub fn host_platform() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("macos", "aarch64") => "darwin-arm64",
        ("windows", "x86_64") => "win-x64",
        ("windows", "aarch64") => "win-arm64",
        _ => return None,
    })
}

/// The platform of a WSL distribution: Linux on this computer's processor.
pub fn wsl_platform() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("linux-x64"),
        "aarch64" => Some("linux-arm64"),
        _ => None,
    }
}

/// The folder of the servers Light installs on this computer: `LIGHT_SERVERS_DIR`; beside the settings of a run with
/// `LIGHT_CONFIG_DIR` (the tests); else in the user's local data (`%LOCALAPPDATA%\Light\servers`,
/// `~/Library/Application Support/Light/servers`, `~/.local/share/Light/servers`).
pub fn host_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("LIGHT_SERVERS_DIR") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("LIGHT_CONFIG_DIR") {
        return Some(PathBuf::from(d).join("servers"));
    }
    dirs::data_local_dir().map(|d| d.join("Light").join("servers"))
}

/// The folder of the servers inside a WSL distribution, as a shell word (`$HOME` is the project user's):
/// `LIGHT_WSL_SERVERS_DIR`, else `$HOME/.local/share/light/servers`.
pub fn wsl_dir() -> String {
    checked_wsl_dir(std::env::var("LIGHT_WSL_SERVERS_DIR").ok())
}

/// A folder for the servers in Linux, if it is one: absolute (or under `$HOME`), and nothing a shell would take for
/// more than a path inside double quotes. Anything else is the default: a relative folder would land wherever the
/// command happens to start.
fn checked_wsl_dir(dir: Option<String>) -> String {
    match dir.map(|d| d.trim().trim_end_matches('/').to_string()) {
        Some(d) if (d.starts_with('/') || d == "$HOME" || d.starts_with("$HOME/")) && !d.contains(['"', '`', '\\', '\n']) && !d[1..].contains('$') => d,
        _ => "$HOME/.local/share/light/servers".into(),
    }
}

/// Where servers are installed and run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// This computer, in this folder.
    Host(PathBuf),
    /// A WSL distribution, as `user` (none: its default user), in `dir` (see [`wsl_dir`]); `mount_root` is where it
    /// sees the drives of this computer (`/mnt/`).
    Wsl { distro: String, user: Option<String>, dir: String, mount_root: String },
}

/// How a unit is made.
#[derive(Clone, Debug, PartialEq)]
pub enum How {
    /// A release archive, unpacked.
    Archive(ArchiveDef),
    /// `npm ci` of the module's `package.json` and `package-lock.json`, with the runtime installed in the folder
    /// `runtime` (its programs in `runtime_bin` inside it).
    Npm { package_json: PathBuf, lock: PathBuf, runtime: String, runtime_bin: String },
}

/// One thing to install: the runtime or the server.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    /// What the user is told: `node 24.22.2`, `basedpyright 1.40.2`.
    pub title: String,
    /// Its folder among the servers.
    pub folder: String,
    pub source: String,
    pub size: u64,
    pub how: How,
}

/// What installing the server `def` of the module in `module_dir` on `platform` takes: its runtime first, if it has
/// one, then the server.
pub fn plan(def: &LspDef, module_dir: &Path, platform: &str) -> Result<Vec<Unit>, String> {
    let Some(install) = &def.install else { return Err(format!("{} is not installed by Light", def.name)) };
    if !install.available_for(platform) {
        return Err(format!("{} cannot be installed by Light on {platform}", def.name));
    }
    let mut units = vec![];
    let folder = install.folder(&def.name);
    let title = format!("{} {}", def.name, install.version);
    if install.npm.is_empty() {
        let a = install.archives[platform].clone();
        units.push(Unit { title, folder, source: install.source.clone(), size: a.size.max(install.size), how: How::Archive(a) });
        return Ok(units);
    }
    let Some(r) = &install.runtime else { return Err(format!("{} names no runtime", def.name)) };
    let a = r.archives[platform].clone();
    let runtime_bin = a.bin.clone();
    units.push(Unit { title: format!("{} {}", r.name, r.version), folder: r.folder(), source: r.source.clone(), size: a.size, how: How::Archive(a) });
    let package_json = module_dir.join(format!("{}.package.json", install.npm));
    let lock = module_dir.join(format!("{}.package-lock.json", install.npm));
    for f in [&package_json, &lock] {
        if !f.is_file() {
            return Err(format!("the module has no {}", f.display()));
        }
    }
    units.push(Unit { title, folder, source: install.source.clone(), size: install.size, how: How::Npm { package_json, lock, runtime: r.folder(), runtime_bin } });
    Ok(units)
}

/// The folders of programs to put first on PATH for `def` on this computer, from the folder of its servers `dir`:
/// the server's and its runtime's, when the server is installed there (nothing otherwise).
pub fn host_paths(def: &LspDef, dir: &Path, platform: &str) -> Vec<PathBuf> {
    let Some(install) = &def.install else { return vec![] };
    let home = dir.join(install.folder(&def.name));
    if !home.join(STAMP).is_file() {
        return vec![];
    }
    let mut out = vec![join_bin(&home, &install.bin(platform))];
    if let Some(r) = &install.runtime
        && let Some(a) = r.archives.get(platform)
    {
        out.push(join_bin(&dir.join(r.folder()), &a.bin));
    }
    out
}

fn join_bin(home: &Path, bin: &str) -> PathBuf {
    bin.split('/').filter(|p| !p.is_empty()).fold(home.to_path_buf(), |p, c| p.join(c))
}

/// The shell line that starts `def` in a WSL distribution: with the server's and its runtime's folders first on PATH
/// when it is installed in `dir` (see [`wsl_dir`]), else as it is.
pub fn wsl_line(def: &LspDef, dir: &str, platform: &str, line: &str) -> String {
    let Some(install) = def.install.as_ref().filter(|i| i.available_for(platform)) else { return line.to_string() };
    let home = format!("{dir}/{}", install.folder(&def.name));
    let mut path = vec![bin_in(&home, &install.bin(platform))];
    if let Some(r) = &install.runtime
        && let Some(a) = r.archives.get(platform)
    {
        path.push(bin_in(&format!("{dir}/{}", r.folder()), &a.bin));
    }
    // folders and versions are plain (the manifest checks them); `$HOME` in `dir` is the shell's to expand
    format!("if [ -f \"{home}/{STAMP}\" ]; then PATH=\"{}:$PATH\"; export PATH; fi; {line}", path.join(":"))
}

fn bin_in(home: &str, bin: &str) -> String {
    if bin.is_empty() { home.to_string() } else { format!("{home}/{bin}") }
}

/// A path of this computer as a WSL distribution sees it: `E:\a\b` is `/mnt/e/a/b`.
fn linux_path(p: &Path, mount_root: &str) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        let root = if mount_root.ends_with('/') { mount_root.to_string() } else { format!("{mount_root}/") };
        return format!("{root}{}{}", (b[0] as char).to_ascii_lowercase(), &s[2..]);
    }
    s
}

fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.stdin(Stdio::null())
}

/// Run `cmd`; what it printed goes to `log`. An error says what failed, with the end of what it printed.
fn exec(mut cmd: Command, what: &str, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    let out = quiet(&mut cmd).output().map_err(|e| format!("{what}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        log(l);
    }
    if out.status.success() {
        return Ok(());
    }
    let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = tail[tail.len().saturating_sub(3)..].join(" / ");
    Err(if tail.is_empty() { format!("{what} failed") } else { format!("{what} failed: {tail}") })
}

/// A shell script run at a WSL place, in the user's home, with arguments `$1`, `$2`... (never put into the script
/// itself).
fn wsl_sh(place: &Place, script: &str, args: &[&str]) -> Command {
    let Place::Wsl { distro, user, .. } = place else { unreachable!("a WSL place") };
    let mut c = crate::wsl::command_as_user(distro, user.as_deref(), Some("~"), "sh");
    c.args(["-c", script, "sh"]).args(args);
    c
}

/// Is the unit installed at the place?
fn installed(unit: &Unit, place: &Place) -> bool {
    match place {
        Place::Host(dir) => dir.join(&unit.folder).join(STAMP).is_file(),
        Place::Wsl { dir, .. } => {
            let mut c = wsl_sh(place, &format!("test -f \"{dir}/$1/{STAMP}\""), &[&unit.folder]);
            quiet(&mut c).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        }
    }
}

/// Download `a` into `downloads` (unless a checked copy is there) and check its SHA-256: the checked file.
fn fetch(a: &ArchiveDef, downloads: &Path, log: &mut dyn FnMut(&str)) -> Result<PathBuf, String> {
    let name = a.url.rsplit('/').next().filter(|n| !n.is_empty() && !n.contains(['\\', ':'])).ok_or_else(|| format!("{}: no file name", a.url))?;
    std::fs::create_dir_all(downloads).map_err(|e| format!("{}: {e}", downloads.display()))?;
    let file = downloads.join(format!("{}-{name}", &a.sha256[..12]));
    let check = |f: &Path| std::fs::read(f).is_ok_and(|data| sha256_hex(&data).eq_ignore_ascii_case(&a.sha256));
    if check(&file) {
        log(&format!("{name}: downloaded before, SHA-256 checked"));
        return Ok(file);
    }
    let part = file.with_extension("part");
    log(&format!("downloading {}", a.url));
    let mut c = Command::new("curl");
    let limit = (a.size.max(1 << 20) * 2).to_string();
    c.args(["-fsSL", "--proto", "=https", "--proto-redir", "=https", "--connect-timeout", "20", "--max-time", "1800", "--max-filesize", &limit, "-A", "Light", "-o"]).arg(&part).arg(&a.url);
    exec(c, "download", log)?;
    if !check(&part) {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{name}: the SHA-256 is not the one the module names - the file was thrown away"));
    }
    std::fs::rename(&part, &file).map_err(|e| format!("{}: {e}", file.display()))?;
    log(&format!("{name}: SHA-256 {} checked", a.sha256));
    Ok(file)
}

/// `tar` of this computer: on Windows the system's own (one from Git on PATH reads `C:` as a host name).
fn host_tar() -> Command {
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        return Command::new(Path::new(&root).join("System32").join("tar.exe"));
    }
    Command::new("tar")
}

/// Make one unit at the place, in `<folder>.part`, then move it into place and stamp it.
fn make(unit: &Unit, place: &Place, downloads: &Path, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    match place {
        Place::Host(dir) => {
            let part = dir.join(format!("{}.part", unit.folder));
            let home = dir.join(&unit.folder);
            let _ = std::fs::remove_dir_all(&part);
            std::fs::create_dir_all(&part).map_err(|e| format!("{}: {e}", part.display()))?;
            match &unit.how {
                How::Archive(a) => {
                    let file = fetch(a, downloads, log)?;
                    let mut c = host_tar();
                    c.arg("-xf").arg(&file).arg("-C").arg(&part).arg(format!("--strip-components={}", a.strip));
                    exec(c, "unpacking", log)?;
                }
                How::Npm { package_json, lock, runtime, runtime_bin } => {
                    for (from, to) in [(package_json, "package.json"), (lock, "package-lock.json")] {
                        std::fs::copy(from, part.join(to)).map_err(|e| format!("{}: {e}", from.display()))?;
                    }
                    let node = join_bin(&dir.join(runtime), runtime_bin);
                    let mut c = Command::new(node.join(if cfg!(windows) { "npm.cmd" } else { "npm" }));
                    let path = std::env::join_paths(std::iter::once(node.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).map_err(|e| e.to_string())?;
                    c.args(["ci", "--ignore-scripts", "--no-audit", "--no-fund", "--omit=dev"]).current_dir(&part).env("PATH", path).env("npm_config_cache", dir.join(".npm-cache")).env("npm_config_update_notifier", "false");
                    log(&format!("npm ci in {}", part.display()));
                    exec(c, "npm ci", log)?;
                }
            }
            let _ = std::fs::remove_dir_all(&home);
            std::fs::rename(&part, &home).map_err(|e| format!("{}: {e}", home.display()))?;
            std::fs::write(home.join(STAMP), &unit.title).map_err(|e| format!("{}: {e}", home.display()))
        }
        Place::Wsl { dir, mount_root, .. } => {
            let part = format!("{dir}/{}.part", unit.folder);
            let home = format!("{dir}/{}", unit.folder);
            match &unit.how {
                How::Archive(a) => {
                    let file = fetch(a, downloads, log)?;
                    let script = format!("rm -rf \"{part}\" && mkdir -p \"{part}\" && tar -xf \"$1\" -C \"{part}\" --strip-components=\"$2\"");
                    exec(wsl_sh(place, &script, &[&linux_path(&file, mount_root), &a.strip.to_string()]), "unpacking", log)?;
                }
                How::Npm { package_json, lock, runtime, runtime_bin } => {
                    let node = bin_in(&format!("{dir}/{runtime}"), runtime_bin);
                    let script = format!(
                        "rm -rf \"{part}\" && mkdir -p \"{part}\" && cp \"$1\" \"{part}/package.json\" && cp \"$2\" \"{part}/package-lock.json\" && cd \"{part}\" && PATH=\"{node}:$PATH\" npm_config_cache=\"{dir}/.npm-cache\" npm_config_update_notifier=false npm ci --ignore-scripts --no-audit --no-fund --omit=dev"
                    );
                    log(&format!("npm ci in {part}"));
                    exec(wsl_sh(place, &script, &[&linux_path(package_json, mount_root), &linux_path(lock, mount_root)]), "npm ci", log)?;
                }
            }
            let script = format!("rm -rf \"{home}\" && mv \"{part}\" \"{home}\" && printf '%s' \"$1\" > \"{home}/{STAMP}\"");
            exec(wsl_sh(place, &script, &[&unit.title]), "finishing", log)
        }
    }
}

/// Install `units` at `place`, in order; one already installed is left as it is. `downloads` is a folder of this
/// computer the archives are downloaded to (kept, so that another install does not download them again).
pub fn run(units: &[Unit], place: &Place, downloads: &Path, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    for u in units {
        if installed(u, place) {
            log(&format!("{}: installed already", u.title));
            continue;
        }
        log(&format!("installing {} from {}", u.title, u.source));
        make(u, place, downloads, log).map_err(|e| format!("{}: {e}", u.title))?;
        log(&format!("{}: installed", u.title));
    }
    Ok(())
}

/// Remove the server `def` from the place (its runtime stays: other servers may run on it).
pub fn remove(def: &LspDef, place: &Place) -> Result<(), String> {
    let Some(install) = &def.install else { return Ok(()) };
    let folder = install.folder(&def.name);
    match place {
        Place::Host(dir) => match std::fs::remove_dir_all(dir.join(&folder)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
        Place::Wsl { dir, .. } => exec(wsl_sh(place, &format!("rm -rf \"{dir}/$1\""), &[&folder]), "removing", &mut |_| {}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{InstallDef, RuntimeDef};
    use crate::testutil::TempDir;
    use std::collections::BTreeMap;

    fn archive(url: &str, sha256: &str, strip: u32, bin: &str) -> ArchiveDef {
        ArchiveDef { url: url.into(), sha256: sha256.into(), size: 1000, strip, bin: bin.into() }
    }

    fn npm_server(dir: &Path) -> LspDef {
        std::fs::write(dir.join("basedpyright.package.json"), "{}").unwrap();
        std::fs::write(dir.join("basedpyright.package-lock.json"), "{}").unwrap();
        let mut node = BTreeMap::new();
        node.insert("linux-x64".to_string(), archive("https://nodejs.org/dist/v24.22.2/node-v24.22.2-linux-x64.tar.gz", &"a".repeat(64), 1, "bin"));
        node.insert("win-x64".to_string(), archive("https://nodejs.org/dist/v24.22.2/node-v24.22.2-win-x64.zip", &"b".repeat(64), 1, ""));
        LspDef {
            name: "basedpyright".into(),
            command: "basedpyright-langserver --stdio".into(),
            install: Some(InstallDef {
                version: "1.40.2".into(),
                source: "registry.npmjs.org".into(),
                size: 36_000_000,
                runtime: Some(RuntimeDef { name: "node".into(), version: "24.22.2".into(), source: "nodejs.org".into(), archives: node }),
                npm: "basedpyright".into(),
                archives: BTreeMap::new(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn this_computer_has_a_platform() {
        assert!(crate::modules::PLATFORMS.contains(&host_platform().unwrap()));
    }

    #[test]
    fn an_npm_server_comes_after_its_runtime() {
        let d = TempDir::new("install-plan");
        let def = npm_server(d.path());
        let units = plan(&def, d.path(), "linux-x64").unwrap();
        assert_eq!(units.iter().map(|u| (u.title.as_str(), u.folder.as_str())).collect::<Vec<_>>(), [("node 24.22.2", "node-24.22.2"), ("basedpyright 1.40.2", "basedpyright-1.40.2")]);
        match &units[1].how {
            How::Npm { runtime, runtime_bin, lock, .. } => assert_eq!((runtime.as_str(), runtime_bin.as_str(), lock.file_name().unwrap().to_str().unwrap()), ("node-24.22.2", "bin", "basedpyright.package-lock.json")),
            h => panic!("{h:?}"),
        }
        assert!(plan(&def, d.path(), "darwin-arm64").unwrap_err().contains("darwin-arm64"));
        std::fs::remove_file(d.path().join("basedpyright.package-lock.json")).unwrap();
        assert!(plan(&def, d.path(), "linux-x64").unwrap_err().contains("package-lock"));
    }

    #[test]
    fn an_installed_server_comes_first_on_path() {
        let d = TempDir::new("install-paths");
        let def = npm_server(d.path());
        assert!(host_paths(&def, d.path(), "win-x64").is_empty(), "not installed");
        std::fs::create_dir_all(d.path().join("basedpyright-1.40.2")).unwrap();
        std::fs::write(d.path().join("basedpyright-1.40.2").join(STAMP), "x").unwrap();
        assert_eq!(host_paths(&def, d.path(), "win-x64"), [d.path().join("basedpyright-1.40.2").join("node_modules").join(".bin"), d.path().join("node-24.22.2")]);
        assert_eq!(host_paths(&def, d.path(), "linux-x64")[1], d.path().join("node-24.22.2").join("bin"));
        let line = wsl_line(&def, "$HOME/.local/share/light/servers", "linux-x64", "basedpyright-langserver --stdio");
        assert_eq!(
            line,
            "if [ -f \"$HOME/.local/share/light/servers/basedpyright-1.40.2/.light-installed\" ]; then PATH=\"$HOME/.local/share/light/servers/basedpyright-1.40.2/node_modules/.bin:$HOME/.local/share/light/servers/node-24.22.2/bin:$PATH\"; export PATH; fi; basedpyright-langserver --stdio"
        );
        // a server Light does not install starts as it is
        assert_eq!(wsl_line(&LspDef::default(), "/x", "linux-x64", "clangd"), "clangd");
    }

    #[test]
    fn the_folder_in_linux_is_absolute_and_plain() {
        let default = "$HOME/.local/share/light/servers";
        assert_eq!(checked_wsl_dir(None), default);
        assert_eq!(checked_wsl_dir(Some("/mnt/e/servers/".into())), "/mnt/e/servers");
        assert_eq!(checked_wsl_dir(Some("$HOME/srv".into())), "$HOME/srv");
        // what Git Bash makes of /mnt/e/... for a Windows program, and other things that are no such folder
        for bad in ["C:/Program Files/Git/mnt/e/servers", "servers", "/x\"; rm -rf ~", "/x/$(id)", "/x/`id`", "$HOME$PATH", ""] {
            assert_eq!(checked_wsl_dir(Some(bad.into())), default, "{bad}");
        }
    }

    #[test]
    fn wsl_sees_the_drives_under_its_mount_root() {
        assert_eq!(linux_path(Path::new(r"E:\Light\downloads\a.tar.gz"), "/mnt/"), "/mnt/e/Light/downloads/a.tar.gz");
        assert_eq!(linux_path(Path::new(r"C:\x"), "/win"), "/win/c/x");
    }

    /// The whole of an archive install on this computer, from a `file://` archive: downloaded, checked, unpacked into
    /// place and stamped; a second run leaves it; a wrong SHA-256 installs nothing.
    #[test]
    fn an_archive_is_installed_checked_and_only_once() {
        let d = TempDir::new("install-run");
        let src = d.path().join("src").join("asm-lsp-1");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin").join("asm-lsp"), "the program").unwrap();
        let tgz = d.path().join("asm-lsp.tar.gz");
        let mut tar = host_tar();
        tar.arg("-czf").arg(&tgz).arg("-C").arg(d.path().join("src")).arg("asm-lsp-1");
        assert!(quiet(&mut tar).status().unwrap().success());
        let sha = sha256_hex(&std::fs::read(&tgz).unwrap());
        let url = format!("file:///{}", tgz.to_string_lossy().replace('\\', "/").trim_start_matches('/'));
        let unit = |sha: &str| Unit { title: "asm-lsp 1".into(), folder: "asm-lsp-1".into(), source: "here".into(), size: 1000, how: How::Archive(archive(&url, sha, 1, "bin")) };
        let servers = d.path().join("servers");
        let place = Place::Host(servers.clone());
        let downloads = d.path().join("downloads");
        // `--proto =https` keeps the real downloads to https; this one is a file
        let mut log = vec![];
        let r = run(&[unit(&sha)], &place, &downloads, &mut |l| log.push(l.to_string()));
        if let Err(e) = &r {
            assert!(e.contains("download"), "{e}");
            // the file:// address is refused by --proto: check the rest with the archive put where a download goes
            std::fs::create_dir_all(&downloads).unwrap();
            std::fs::copy(&tgz, downloads.join(format!("{}-asm-lsp.tar.gz", &sha[..12]))).unwrap();
            run(&[unit(&sha)], &place, &downloads, &mut |l| log.push(l.to_string())).unwrap();
        }
        assert_eq!(std::fs::read_to_string(servers.join("asm-lsp-1").join("bin").join("asm-lsp")).unwrap(), "the program");
        assert!(servers.join("asm-lsp-1").join(STAMP).is_file() && !servers.join("asm-lsp-1.part").exists());
        let mut again = vec![];
        run(&[unit(&sha)], &place, &downloads, &mut |l| again.push(l.to_string())).unwrap();
        assert!(again.iter().any(|l| l.contains("installed already")), "{again:?}");
        // a wrong hash: the download is thrown away, nothing is installed
        let other = Unit { folder: "other-1".into(), ..unit(&"0".repeat(64)) };
        let err = run(&[other], &place, &d.path().join("fresh"), &mut |_| {}).unwrap_err();
        assert!(!servers.join("other-1").exists(), "{err}");
    }
}

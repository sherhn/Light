//! Windows Subsystem for Linux: finding it and its distributions, setting them up, and running programs inside a
//! distribution with Windows paths translated to Linux ones.
//!
//! Every call goes through `wsl.exe` (in the UI tests through the fake script named by `LIGHT_FAKE_WSL`). The
//! setup commands only ever add things - WSL itself, a distribution, packages inside one; nothing here unregisters,
//! moves or deletes a distribution or its files.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Where Windows drives appear inside a distribution unless `/etc/wsl.conf` says otherwise.
pub const DEFAULT_MOUNT_ROOT: &str = "/mnt/";
/// The distribution suggested to someone who has none: the current Ubuntu LTS.
pub const RECOMMENDED_DISTRO: &str = "Ubuntu-24.04";

/// A distribution to run programs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub distro: String,
    /// Prefix of the Windows drives, with a trailing slash (`/mnt/`).
    pub mount_root: String,
}

impl Target {
    pub fn new(distro: impl Into<String>, mount_root: &str) -> Target {
        let mut m = mount_root.trim().to_string();
        if m.is_empty() {
            m = DEFAULT_MOUNT_ROOT.to_string();
        }
        if !m.ends_with('/') {
            m.push('/');
        }
        Target { distro: distro.into(), mount_root: m }
    }
}

// ---------------------------------------------------------------------------------------------- paths

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)).then(|| &s[prefix.len()..])
}

/// The path `p` (a Windows path) as programs inside the distribution see it: `C:\a\b` -> `/mnt/c/a/b`,
/// `\\wsl.localhost\Ubuntu\home\me` -> `/home/me`. Relative paths only get forward slashes.
pub fn to_linux(p: &Path, t: &Target) -> String {
    let raw = p.to_string_lossy().replace('\\', "/");
    let s = raw.strip_prefix("//?/").unwrap_or(&raw);
    for host in ["//wsl.localhost/", "//wsl$/"] {
        if let Some(rest) = strip_prefix_ci(s, host) {
            let inside = rest.split_once('/').map_or("", |(_, r)| r);
            return format!("/{inside}");
        }
    }
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        let drive = (b[0] as char).to_ascii_lowercase();
        let rest = s[2..].trim_start_matches('/');
        return format!("{}{drive}{}{rest}", t.mount_root, if rest.is_empty() { "" } else { "/" });
    }
    s.to_string()
}

/// The Windows path of a path inside the distribution: `/mnt/c/a` -> `C:\a`, `/home/me` -> `\\wsl.localhost\<distro>\home\me`.
pub fn to_windows(linux: &str, t: &Target) -> PathBuf {
    if let Some(rest) = linux.strip_prefix(t.mount_root.as_str()) {
        let (drive, tail) = rest.split_once('/').unwrap_or((rest, ""));
        if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() {
            return PathBuf::from(format!("{}:\\{}", drive.to_ascii_uppercase(), tail.replace('/', "\\")));
        }
    }
    if linux.starts_with('/') {
        return PathBuf::from(format!("\\\\wsl.localhost\\{}{}", t.distro, linux.replace('/', "\\")));
    }
    PathBuf::from(linux)
}

/// Quote `s` for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ---------------------------------------------------------------------------------------------- wsl.exe

/// Is `wsl.exe` replaced by the fake of the UI tests?
pub fn is_faked() -> bool {
    std::env::var_os("LIGHT_FAKE_WSL").or_else(|| std::env::var_os("AIR_FAKE_WSL")).is_some()
}

/// `wsl.exe` with its own messages in UTF-8 (`WSL_UTF8`). `LIGHT_FAKE_WSL` (a Python script, run by
/// `LIGHT_FAKE_WSL_PYTHON`) stands in for it in the UI tests, so they never touch the real WSL.
pub fn wsl_command() -> Command {
    let mut c = match std::env::var_os("LIGHT_FAKE_WSL").or_else(|| std::env::var_os("AIR_FAKE_WSL")) {
        Some(script) => {
            let mut c = Command::new(std::env::var_os("LIGHT_FAKE_WSL_PYTHON").or_else(|| std::env::var_os("AIR_FAKE_WSL_PYTHON")).unwrap_or_else(|| "python".into()));
            c.arg(script);
            c
        }
        None => Command::new("wsl.exe"),
    };
    c.env("WSL_UTF8", "1");
    crate::no_window(&mut c);
    c
}

/// Text written by `wsl.exe`: UTF-8 when `WSL_UTF8` is honoured, UTF-16LE from older builds.
pub fn decode(bytes: &[u8]) -> String {
    let nul = bytes.iter().filter(|b| **b == 0).count();
    if bytes.len() >= 2 && nul * 4 >= bytes.len() {
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units).trim_start_matches('\u{feff}').to_string();
    }
    String::from_utf8_lossy(bytes).replace('\0', "")
}

/// Output of a finished command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Captured {
    pub text: String,
    pub code: Option<i32>,
    pub timed_out: bool,
}

impl Captured {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
}

/// Run `cmd` to completion (killed after `timeout`), stdout and stderr together.
pub fn capture(mut cmd: Command, timeout: Duration) -> io::Result<Captured> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut readers = Vec::new();
    for mut stream in [Box::new(child.stdout.take().expect("piped")) as Box<dyn Read + Send>, Box::new(child.stderr.take().expect("piped"))] {
        readers.push(std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf);
            buf
        }));
    }
    let deadline = Instant::now() + timeout;
    let (code, timed_out) = loop {
        match child.try_wait()? {
            Some(status) => break (status.code(), false),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    let mut bytes = Vec::new();
    for r in readers {
        bytes.extend(r.join().unwrap_or_default());
    }
    Ok(Captured { text: decode(&bytes), code, timed_out })
}

fn wsl_capture(args: &[&str], timeout: Duration) -> io::Result<Captured> {
    let mut c = wsl_command();
    c.args(args);
    capture(c, timeout)
}

// ---------------------------------------------------------------------------------------------- detection

/// An installed distribution, as `wsl --list --verbose` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distro {
    pub name: String,
    /// "Running" / "Stopped" in the language of Windows; only for display.
    pub state: String,
    pub version: u8,
    pub default: bool,
}

/// A distribution that can be installed (`wsl --list --online`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnlineDistro {
    pub name: String,
    pub friendly: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Presence {
    /// There is no `wsl.exe`: Windows is too old for WSL.
    Unsupported,
    /// `wsl.exe` exists but WSL itself is not installed / enabled yet.
    NotInstalled,
    Installed { version: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub presence: Presence,
    pub distros: Vec<Distro>,
}

impl Probe {
    pub fn default_distro(&self) -> Option<&Distro> {
        self.distros.iter().find(|d| d.default).or(self.distros.first())
    }
}

/// The WSL version from `wsl --version` (first dotted number of the line that names WSL; the labels are localized).
pub fn parse_version(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains("WSL"))?;
    line.split(|c: char| c.is_whitespace() || c == ':')
        .find(|tok| tok.split('.').count() >= 3 && tok.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())))
        .map(str::to_string)
}

/// Rows of `wsl --list --verbose`: `* Name  State  2`. The header and any message lines are skipped; only rows that
/// end in a WSL version number count, so localized headers do not matter.
pub fn parse_list_verbose(text: &str) -> Vec<Distro> {
    text.lines()
        .filter_map(|line| {
            let t = line.trim();
            let default = t.starts_with('*');
            let tokens: Vec<&str> = t.trim_start_matches('*').split_whitespace().collect();
            if tokens.len() < 3 {
                return None;
            }
            let version: u8 = tokens.last()?.parse().ok().filter(|v| (1..=2).contains(v))?;
            Some(Distro { name: tokens[0].to_string(), state: tokens[1..tokens.len() - 1].join(" "), version, default })
        })
        .collect()
}

/// Rows of `wsl --list --online` after its `NAME  FRIENDLY NAME` header.
pub fn parse_online(text: &str) -> Vec<OnlineDistro> {
    let mut lines = text.lines().skip_while(|l| l.split_whitespace().next() != Some("NAME"));
    if lines.next().is_none() {
        return vec![];
    }
    lines
        .filter_map(|l| {
            let l = l.trim();
            let (name, friendly) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            (!name.is_empty()).then(|| OnlineDistro { name: name.to_string(), friendly: friendly.trim().to_string() })
        })
        .collect()
}

/// Ask WSL what is there. Blocking (starts no distribution): run it off the UI thread.
pub fn probe() -> Probe {
    let short = Duration::from_secs(30);
    let version = match wsl_capture(&["--version"], short) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Probe { presence: Presence::Unsupported, distros: vec![] },
        Err(_) => None,
        Ok(c) => c.ok().then(|| parse_version(&c.text)).flatten(),
    };
    let presence = match version {
        Some(v) => Presence::Installed { version: Some(v) },
        // older, inbox WSL has no --version; it answers --status only when it is set up
        None => match wsl_capture(&["--status"], short) {
            Ok(c) if c.ok() => Presence::Installed { version: None },
            Ok(_) => Presence::NotInstalled,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Presence::Unsupported,
            Err(_) => Presence::NotInstalled,
        },
    };
    let distros = if matches!(presence, Presence::Installed { .. }) { wsl_capture(&["--list", "--verbose"], short).map(|c| parse_list_verbose(&c.text)).unwrap_or_default() } else { vec![] };
    Probe { presence, distros }
}

/// Distributions that can be installed. Needs the internet.
pub fn list_online() -> Result<Vec<OnlineDistro>, String> {
    let c = wsl_capture(&["--list", "--online"], Duration::from_secs(60)).map_err(|e| e.to_string())?;
    let list = parse_online(&c.text);
    if list.is_empty() { Err(c.text.trim().to_string()) } else { Ok(list) }
}

/// What a distribution offers for running Lisp.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SbclCheck {
    /// Absolute path of SBCL inside the distribution.
    pub path: Option<String>,
    pub version: Option<String>,
    /// Where the Windows drives are mounted (from `wslpath`), with a trailing slash.
    pub mount_root: Option<String>,
}

/// Script that reports SBCL and the drive mount point as `KEY=value` lines (run in a login shell so a
/// user-installed SBCL on the profile's PATH is found too).
pub fn sbcl_check_script(sbcl: &str) -> String {
    let sbcl = sh_quote(if sbcl.trim().is_empty() { "sbcl" } else { sbcl.trim() });
    format!("p=$(command -v {sbcl} 2>/dev/null); echo \"LIGHT_SBCL=$p\"; if [ -n \"$p\" ]; then echo \"LIGHT_VERSION=$(\"$p\" --version 2>/dev/null)\"; fi; echo \"LIGHT_MOUNT=$(wslpath -u 'C:\\' 2>/dev/null)\"")
}

pub fn parse_sbcl_check(text: &str) -> SbclCheck {
    let value = |key: &str| text.lines().find_map(|l| l.trim().strip_prefix(key)).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    let mount_root = value("LIGHT_MOUNT=").and_then(|m| {
        // `/mnt/c/` -> `/mnt/`
        let t = m.trim_end_matches('/');
        let root = &t[..t.len().saturating_sub(1)];
        (t.ends_with(['c', 'C']) && root.ends_with('/')).then(|| root.to_string())
    });
    SbclCheck { path: value("LIGHT_SBCL="), version: value("LIGHT_VERSION="), mount_root }
}

/// Look for SBCL inside `distro` (starts the distribution if it is stopped).
pub fn check_sbcl(distro: &str, sbcl: &str) -> Result<SbclCheck, String> {
    let mut c = wsl_command();
    c.args(["-d", distro, "-e", "sh", "-lc", &sbcl_check_script(sbcl)]);
    let out = capture(c, Duration::from_secs(90)).map_err(|e| e.to_string())?;
    if out.timed_out {
        return Err("the distribution did not answer in time".into());
    }
    let check = parse_sbcl_check(&out.text);
    if check.path.is_none() && check.mount_root.is_none() && !out.ok() {
        return Err(out.text.trim().to_string());
    }
    Ok(check)
}

// ---------------------------------------------------------------------------------------------- setup

/// Kinds of files a distribution can be installed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// A WSL distribution package (`.wsl`).
    Wsl,
    /// A root file system archive (`.tar`, `.tar.gz`, `.tar.xz`, `.tgz`).
    Tar,
    /// An installer disk for a PC or a virtual machine: WSL cannot use it.
    Iso,
    Unknown,
}

pub fn image_kind(p: &Path) -> ImageKind {
    let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if name.ends_with(".wsl") {
        ImageKind::Wsl
    } else if [".tar", ".tar.gz", ".tgz", ".tar.xz", ".tar.zst"].iter().any(|e| name.ends_with(e)) {
        ImageKind::Tar
    } else if name.ends_with(".iso") || name.ends_with(".img") {
        ImageKind::Iso
    } else {
        ImageKind::Unknown
    }
}

/// Install WSL itself (no distribution). Needs administrator rights: Windows asks for them (UAC) and shows the
/// installer's own console window. A restart of Windows may be requested afterwards.
pub fn install_wsl_command() -> Command {
    if is_faked() {
        let mut c = wsl_command();
        c.args(["--install", "--no-distribution"]);
        return c;
    }
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", "Start-Process -FilePath wsl.exe -ArgumentList '--install','--no-distribution' -Verb RunAs -Wait"]);
    crate::no_window(&mut c);
    c
}

/// Download and register a distribution (`wsl --install -d NAME --no-launch --web-download [--location DIR]`).
pub fn install_distro_command(name: &str, location: Option<&Path>) -> Command {
    let mut c = wsl_command();
    c.args(["--install", "-d", name, "--no-launch", "--web-download"]);
    if let Some(dir) = location {
        c.arg("--location").arg(dir);
    }
    c
}

/// Register a distribution from a downloaded image. `.iso` files are refused: they are not WSL images.
pub fn install_file_command(file: &Path, name: &str, location: &Path) -> Result<Command, ImageKind> {
    let mut c = wsl_command();
    match image_kind(file) {
        ImageKind::Wsl => {
            c.args(["--install", "--no-launch", "--from-file"]).arg(file).args(["--name", name, "--location"]).arg(location);
        }
        ImageKind::Tar => {
            c.args(["--import", name]).arg(location).arg(file);
        }
        other => return Err(other),
    }
    Ok(c)
}

/// Shell script installing SBCL with the distribution's package manager (run as root). The `@@light stage` lines
/// tell the setup page which phase it is in (see [`crate::setup_progress`]).
pub const INSTALL_SBCL_SCRIPT: &str = "set -e
echo '@@light stage prepare'
if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  echo '@@light stage install'
  apt-get install -y sbcl
elif command -v dnf >/dev/null 2>&1; then
  echo '@@light stage install'
  dnf install -y sbcl
elif command -v zypper >/dev/null 2>&1; then
  echo '@@light stage install'
  zypper --non-interactive install sbcl
elif command -v pacman >/dev/null 2>&1; then
  echo '@@light stage install'
  pacman -Sy --noconfirm sbcl
elif command -v apk >/dev/null 2>&1; then
  echo '@@light stage install'
  apk add sbcl
else
  echo 'No supported package manager (apt, dnf, zypper, pacman, apk) found.' >&2
  exit 2
fi
echo '@@light stage verify'
sbcl --version";

/// Install SBCL inside `distro` as root (`wsl -u root` needs no password).
pub fn install_sbcl_command(distro: &str) -> Command {
    let mut c = wsl_command();
    c.args(["-d", distro, "-u", "root", "-e", "sh", "-c", INSTALL_SBCL_SCRIPT]);
    c
}

/// Programs a C/assembly project built with make needs (os-pi and the like): without one of them the project cannot
/// be built, so these are what the "install" banner is about.
pub const BUILD_TOOLS: [&str; 8] = ["make", "gcc", "ld", "nasm", "gdb", "git", "qemu-system-i386", "qemu-system-x86_64"];

/// A group of the developer tools put into a distribution: its name, the programs that show it is there and the
/// Debian/Ubuntu packages that bring them. The groups come from the modules that are on (C and C++, Python, ...), from
/// [`essential_groups`] and from the tools the user added; Debian and Ubuntu get all of them, the other package
/// managers the core set of [`install_tools_script`]'s other branches (C toolchain, nasm, gdb, git, QEMU) and the
/// packages of the user's own groups.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolGroup {
    pub name: String,
    /// What shows the group is installed: programs on the PATH, or `dpkg:<package>` for a package that brings none
    /// (see [`check_label`]).
    pub programs: Vec<String>,
    pub packages: String,
    /// Added by the user: its packages are named for the distribution's own package manager.
    pub custom: bool,
}

impl ToolGroup {
    pub fn new(name: &str, programs: &[&str], packages: &str) -> ToolGroup {
        ToolGroup { name: name.to_string(), programs: programs.iter().map(|p| p.to_string()).collect(), packages: packages.to_string(), custom: false }
    }

    /// The packages of the group, each once.
    pub fn package_list(&self) -> Vec<&str> {
        let mut v: Vec<&str> = Vec::new();
        for p in self.packages.split_whitespace() {
            if !v.contains(&p) {
                v.push(p);
            }
        }
        v
    }

    /// The group with only the packages `keep` says yes to (its programs stay: they are still looked for).
    pub fn with_packages(&self, keep: impl Fn(&str) -> bool) -> ToolGroup {
        ToolGroup { packages: self.package_list().into_iter().filter(|p| keep(p)).collect::<Vec<_>>().join(" "), ..self.clone() }
    }
}

/// A tool the user adds: `packages` and `programs` are lists separated by spaces. Without programs the packages
/// themselves are looked for (on Debian and Ubuntu with dpkg). `None` when a word of it could do harm in the shell
/// script it ends up in (see [`is_safe_word`]), or the name or the packages are empty.
pub fn custom_group(name: &str, packages: &str, programs: &str) -> Option<ToolGroup> {
    let words = |s: &str| -> Option<Vec<String>> { s.split_whitespace().map(|w| is_safe_word(w).then(|| w.to_string())).collect() };
    let (packages, programs) = (words(packages)?, words(programs)?);
    let name = name.trim();
    if name.is_empty() || packages.is_empty() {
        return None;
    }
    let programs = if programs.is_empty() { packages.iter().map(|p| format!("{DPKG_PREFIX}{p}")).collect() } else { programs };
    Some(ToolGroup { name: name.to_string(), programs, packages: packages.join(" "), custom: true })
}

/// The prefix of a check entry that asks dpkg about a package instead of looking for a program.
const DPKG_PREFIX: &str = "dpkg:";

/// What a check entry is called on screen: a program as it is, a package without the prefix.
pub fn check_label(entry: &str) -> &str {
    entry.strip_prefix(DPKG_PREFIX).unwrap_or(entry)
}

/// A name of a package or a program that is safe to put into a shell script unquoted: letters, digits and `. + - _ :`
/// (what the names of Debian packages are made of), starting with a letter or a digit.
pub fn is_safe_word(w: &str) -> bool {
    w.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_' | ':'))
}

/// What every developer wants whatever the language: git and network tools, text and archive tools, documentation.
pub fn essential_groups() -> Vec<ToolGroup> {
    vec![
        ToolGroup::new("Git and network", &["git", "curl", "wget", "ssh", "rsync"], "git curl wget openssh-client rsync ca-certificates"),
        ToolGroup::new("Files and text", &["zip", "unzip", "xz", "jq", "rg", "fdfind", "tree", "vim", "nano", "htop"], "zip unzip xz-utils jq ripgrep fd-find tree less vim nano htop"),
        ToolGroup::new("Documentation", &["man", "doxygen"], "man-db manpages-dev doxygen"),
    ]
}

/// Every program of `groups`, once, in the order the groups list them.
pub fn all_programs(groups: &[ToolGroup]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for g in groups {
        for p in &g.programs {
            if !v.contains(p) {
                v.push(p.clone());
            }
        }
    }
    v
}

/// The [`BUILD_TOOLS`] a check found missing (only those the checked groups cover: a tool no module asks for is not
/// missed).
pub fn missing_core(found: &[(String, bool)]) -> Vec<String> {
    found.iter().filter(|(n, ok)| !*ok && BUILD_TOOLS.contains(&n.as_str())).map(|(n, _)| n.clone()).collect()
}

/// Script that prints `TOOL=<path>` for every program of the groups (an empty path: missing), and `dpkg:PACKAGE=<status>`
/// for every package that is asked about instead. A login shell, so tools on the user's PATH count too.
pub fn tools_check_script(groups: &[ToolGroup]) -> String {
    all_programs(groups)
        .iter()
        .map(|t| match t.strip_prefix(DPKG_PREFIX) {
            Some(pkg) => format!("echo \"{t}=$(dpkg-query -W -f='${{Status}}' {pkg} 2>/dev/null | grep 'ok installed')\""),
            None => format!("echo \"{t}=$(command -v {t} 2>/dev/null)\""),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Which programs of `groups` the check found (`true`) or missed.
pub fn parse_tools(text: &str, groups: &[ToolGroup]) -> Vec<(String, bool)> {
    all_programs(groups)
        .into_iter()
        .map(|t| {
            let found = text.lines().any(|l| l.trim().strip_prefix(&format!("{t}=")).is_some_and(|v| !v.trim().is_empty()));
            (t, found)
        })
        .collect()
}

/// Look for the developer tools inside `distro`.
pub fn check_tools(distro: &str, groups: &[ToolGroup]) -> Result<Vec<(String, bool)>, String> {
    let mut c = wsl_command();
    c.args(["-d", distro, "-e", "sh", "-lc", &tools_check_script(groups)]);
    let out = capture(c, Duration::from_secs(90)).map_err(|e| e.to_string())?;
    if out.timed_out {
        return Err("the distribution did not answer in time".into());
    }
    let first = all_programs(groups).into_iter().next().unwrap_or_default();
    if !out.text.contains(&format!("{first}=")) {
        return Err(out.text.trim().to_string());
    }
    Ok(parse_tools(&out.text, groups))
}

/// Shell script installing the developer tools (run as root): on Debian and Ubuntu every package of `groups`
/// (packages this release of the distribution does not have are skipped, not fatal); with the other package managers
/// a C toolchain with 32-bit support, nasm, a debugger, git and QEMU (when `groups` has a group that is not the user's
/// own), and the packages of the groups the user added, one by one (one that cannot be installed is skipped).
pub fn install_tools_script(groups: &[ToolGroup]) -> String {
    let mut packages: Vec<&str> = Vec::new();
    for p in groups.iter().flat_map(|g| g.package_list()) {
        if !packages.contains(&p) {
            packages.push(p);
        }
    }
    let apt = packages.join(" ");
    let mut own: Vec<&str> = Vec::new();
    for p in groups.iter().filter(|g| g.custom).flat_map(|g| g.package_list()) {
        if !own.contains(&p) {
            own.push(p);
        }
    }
    let own = own.join(" ");
    let core = groups.iter().any(|g| !g.custom);
    // the way each other package manager installs the core set, then the user's own packages one by one
    let other = |manager: &str, core_cmd: &str, one: &str| -> String {
        let mut s = String::from("  echo '@@light stage install'\n");
        if core {
            s.push_str(&format!("  {core_cmd}\n"));
        }
        if !own.is_empty() {
            s.push_str(&format!("  for p in {own}; do {one} \"$p\" || echo \"Skipped ($p could not be installed with {manager})\"; done\n"));
        }
        s
    };
    let dnf = other("dnf", "dnf install -y make gcc glibc-devel.i686 libgcc.i686 nasm gdb git qemu-system-x86", "dnf install -y");
    let zypper = other("zypper", "zypper --non-interactive install make gcc gcc-32bit nasm gdb git qemu-x86", "zypper --non-interactive install");
    let pacman = other("pacman", "pacman -Sy --noconfirm base-devel lib32-gcc-libs nasm gdb git qemu-system-x86", "pacman -S --noconfirm");
    let apk = other("apk", "apk add build-base nasm gdb git qemu-system-i386 qemu-system-x86_64", "apk add");
    format!(
        "set -e
echo '@@light stage prepare'
if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  echo '@@light stage install'
  have=''
  for p in {apt}; do
    if apt-cache show \"$p\" >/dev/null 2>&1; then have=\"$have $p\"; else echo \"Skipped (this distribution has no $p)\"; fi
  done
  if [ -n \"$have\" ]; then apt-get install -y $have; fi
elif command -v dnf >/dev/null 2>&1; then
{dnf}elif command -v zypper >/dev/null 2>&1; then
{zypper}elif command -v pacman >/dev/null 2>&1; then
{pacman}elif command -v apk >/dev/null 2>&1; then
{apk}else
  echo 'No supported package manager (apt, dnf, zypper, pacman, apk) found.' >&2
  exit 2
fi
echo '@@light stage verify'
for t in make gcc; do
  if command -v \"$t\" >/dev/null 2>&1; then \"$t\" --version | head -n 1; fi
done"
    )
}

/// Install the developer tools inside `distro` as root (`wsl -u root` needs no password).
pub fn install_tools_command(distro: &str, groups: &[ToolGroup]) -> Command {
    let mut c = wsl_command();
    c.args(["-d", distro, "-u", "root", "-e", "sh", "-c", &install_tools_script(groups)]);
    c
}

/// Where Git for Windows keeps its credential manager, as Linux sees it: Linux git can then use the logins already
/// saved on Windows (GitHub included), the way Microsoft's WSL guide sets it up.
pub fn windows_credential_manager(t: &Target) -> Option<String> {
    let candidates = [r"C:\Program Files\Git\mingw64\bin\git-credential-manager.exe", r"C:\Program Files\Git\mingw64\libexec\git-core\git-credential-manager.exe", r"C:\Program Files (x86)\Git\mingw64\bin\git-credential-manager.exe"];
    candidates.iter().map(Path::new).find(|p| p.exists()).map(|p| to_linux(p, t))
}

/// Script for the distribution's default user: Windows git's name and e-mail, and its credential manager.
pub fn git_setup_script(name: &str, email: &str, helper: Option<&str>) -> String {
    let mut lines = vec!["set -e".to_string()];
    if !name.trim().is_empty() {
        lines.push(format!("git config --global user.name {}", sh_quote(name.trim())));
    }
    if !email.trim().is_empty() {
        lines.push(format!("git config --global user.email {}", sh_quote(email.trim())));
    }
    if let Some(h) = helper {
        // the path has spaces: git runs a helper that starts with `!` through the shell, so quote it inside
        lines.push(format!("git config --global credential.helper {}", sh_quote(&format!("!{}", sh_quote(h)))));
    }
    lines.push("git config --global core.autocrlf input".into());
    lines.push("echo \"name: $(git config --global user.name)\"; echo \"email: $(git config --global user.email)\"; echo \"credentials: $(git config --global credential.helper)\"".into());
    lines.join("\n")
}

/// Set up git for the default user of `t`'s distribution (see [`git_setup_script`]).
pub fn git_setup_command(t: &Target, name: &str, email: &str) -> Command {
    let helper = windows_credential_manager(t);
    let mut c = wsl_command();
    c.args(["-d", &t.distro, "-e", "sh", "-lc", &git_setup_script(name, email, helper.as_deref())]);
    c
}

/// The default user's home folder inside `distro` (`/home/me`).
pub fn home_dir(distro: &str) -> Result<String, String> {
    let mut c = wsl_command();
    c.args(["-d", distro, "-e", "sh", "-c", "cd ~ && pwd"]);
    let out = capture(c, Duration::from_secs(60)).map_err(|e| e.to_string())?;
    let home = out.text.lines().map(str::trim).rfind(|l| l.starts_with('/')).map(str::to_string);
    match home {
        Some(h) if out.ok() => Ok(h),
        _ => Err(out.text.trim().to_string()),
    }
}

/// Open an interactive window of `distro` (its first start asks for a Linux user name and password).
pub fn open_window(distro: &str) -> io::Result<()> {
    if is_faked() {
        return Ok(());
    }
    // `start` gives the distribution a console window of its own; the title is the first quoted argument
    Command::new("cmd").args(["/C", "start", "Light", "wsl.exe", "-d", distro, "--cd", "~"]).spawn().map(|_| ())
}

/// Free bytes on the drive of `path` (Windows only; `None` when unknown).
pub fn free_space(path: &Path) -> Option<u64> {
    let s = path.to_string_lossy();
    let drive = s.chars().next().filter(|c| c.is_ascii_alphabetic() && s[1..].starts_with(':'))?;
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", &format!("[System.IO.DriveInfo]::new('{drive}').AvailableFreeSpace")]);
    crate::no_window(&mut c);
    capture(c, Duration::from_secs(20)).ok().filter(Captured::ok).and_then(|o| o.text.trim().parse().ok())
}

// ---------------------------------------------------------------------------------------------- running programs

/// `program args...` inside `t`'s distribution, without a shell (arguments arrive exactly as given), in the Linux
/// counterpart of the Windows directory `cwd`.
pub fn exec(t: &Target, cwd: Option<&Path>, program: &str, args: &[String]) -> Command {
    let mut c = wsl_command();
    c.args(["-d", &t.distro]);
    if let Some(dir) = cwd {
        c.arg("--cd").arg(to_linux(dir, t));
    }
    c.arg("-e").arg(program).args(args);
    c
}

/// A terminal command line run by the distribution's login shell, with extra environment variables. `marker`
/// becomes the shell's `$0` so the command can be found and stopped later (see [`kill_marked`]).
pub fn shell(t: &Target, cwd: &Path, line: &str, env: &[(String, String)], marker: &str) -> Command {
    let mut args: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    args.extend(["sh".to_string(), "-lc".to_string(), line.to_string(), marker.to_string()]);
    exec(t, Some(cwd), "env", &args)
}

static NEXT_MARKER: AtomicU64 = AtomicU64::new(1);

/// A tag put on the command line of a process started in the distribution, unique for this run of Light.
pub fn new_marker() -> String {
    // the trailing `x` keeps `...-1x` from matching `...-10x`
    format!("light-{}-{}x", std::process::id(), NEXT_MARKER.fetch_add(1, Ordering::Relaxed))
}

/// `pgrep` pattern that matches `marker` but not the command line that carries the pattern itself.
pub fn marker_pattern(marker: &str) -> String {
    let mut chars = marker.chars();
    match chars.next() {
        Some(first) => format!("[{first}]{}", chars.as_str()),
        None => String::new(),
    }
}

/// Shell script that ends the processes whose command line matches `$1` and everything started below them (`make`
/// starts `qemu`, which starts a helper, ...): children first, with SIGTERM; what is still there a second later gets
/// SIGKILL.
const KILL_TREE_SCRIPT: &str = "tree() { for c in $(pgrep -P \"$1\" 2>/dev/null); do tree \"$c\"; done; echo \"$1\"; }
all=$(for p in $(pgrep -f -- \"$1\"); do tree \"$p\"; done)
for q in $all; do kill -TERM \"$q\" 2>/dev/null; done
sleep 1
for q in $all; do kill -0 \"$q\" 2>/dev/null && kill -KILL \"$q\" 2>/dev/null; done
true";

/// Stopping `wsl.exe` does not always stop the Linux process it started: end the processes tagged with `marker`
/// and everything they started inside the distribution. Fire and forget.
pub fn kill_marked(t: &Target, marker: &str) {
    let script = KILL_TREE_SCRIPT;
    let mut c = exec(t, None, "sh", &["-c".into(), script.into(), "sh".into(), marker_pattern(marker)]);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let _ = c.spawn();
}

/// `wsl.exe` prints this on every start when Windows has a proxy on localhost, which NAT-mode WSL cannot use: "wsl: A
/// localhost proxy configuration was detected but not mirrored into WSL. WSL in NAT mode does not support localhost
/// proxies." It is about the machine, not the command, so it only clutters terminals and build logs.
fn is_notice(line: &str) -> bool {
    let l = line.trim_start();
    l.len() >= 4 && l[..4].eq_ignore_ascii_case("wsl:") && l.contains("localhost") && l.contains("NAT")
}

/// `text` without the lines `wsl.exe` prints about the host (see [`is_notice`]); other text is returned as it came.
pub fn strip_notices(text: &str) -> String {
    if !text.contains("wsl:") && !text.contains("WSL:") {
        return text.to_string();
    }
    text.split_inclusive('\n').filter(|l| !is_notice(l)).collect()
}

/// Ends the tagged processes of a [`Launch`](crate::discover::Launch) inside the distribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cleanup {
    pub target: Target,
    pub marker: String,
}

impl Cleanup {
    pub fn run(&self) {
        kill_marked(&self.target, &self.marker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Target {
        Target::new("Ubuntu-24.04", "")
    }

    fn args(c: &Command) -> Vec<String> {
        c.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    /// What the C and C++ and Python modules bring, as the app turns their manifests into groups.
    fn module_groups() -> Vec<ToolGroup> {
        let mut g = essential_groups();
        g.push(ToolGroup::new("C and C++", &["make", "gcc", "g++", "ld", "gdb", "cmake"], "build-essential gdb cmake gcc-multilib"));
        g.push(ToolGroup::new("Python", &["python3", "pip3"], "python3 python3-pip python3-venv"));
        g.push(ToolGroup::new("OS development", &["nasm", "qemu-system-i386", "qemu-system-x86_64"], "nasm qemu-system-x86"));
        g.push(ToolGroup::new("Debugging", &["gdb", "valgrind"], "gdb valgrind"));
        g
    }

    #[test]
    fn developer_tools_are_reported_by_group() {
        let groups = module_groups();
        let all = all_programs(&groups);
        assert!(BUILD_TOOLS.iter().all(|t| all.iter().any(|a| a == t)), "every core tool is checked: {all:?}");
        assert_eq!(all.iter().filter(|p| *p == "gdb").count(), 1, "a program in two groups is checked once");
        let text = "make=/usr/bin/make\ngcc=/usr/bin/gcc\nld=/usr/bin/ld\nnasm=\ngdb=\ngit=/usr/bin/git\nqemu-system-i386=\nqemu-system-x86_64=\ncmake=/usr/bin/cmake\n";
        let tools = parse_tools(text, &groups);
        assert_eq!(tools.len(), all.len());
        assert!(tools.contains(&("make".to_string(), true)));
        assert!(tools.contains(&("cmake".to_string(), true)));
        assert!(tools.contains(&("gdb".to_string(), false)));
        assert!(tools.contains(&("valgrind".to_string(), false)), "not printed means not found");
        assert_eq!(missing_core(&tools), ["gdb", "nasm", "qemu-system-i386", "qemu-system-x86_64"], "only the core tools matter for the banner");
        assert!(tools_check_script(&groups).contains("command -v qemu-system-i386") && tools_check_script(&groups).contains("command -v valgrind"));
        // without the modules that bring them the tools are not looked for (nor missed)
        let only_essentials = parse_tools(text, &essential_groups());
        assert!(!only_essentials.iter().any(|(n, _)| n == "make") && missing_core(&only_essentials).is_empty());
    }

    #[test]
    fn the_install_script_brings_every_group() {
        let groups = module_groups();
        let script = install_tools_script(&groups);
        for g in &groups {
            assert!(!g.name.is_empty() && !g.programs.is_empty());
            for pkg in g.packages.split_whitespace() {
                assert!(script.contains(pkg), "{pkg} of {}", g.name);
            }
        }
        let list = &script[script.find("for p in ").unwrap()..script.find("; do").unwrap()];
        assert_eq!(list.split_whitespace().filter(|p| *p == "gdb").count(), 1, "a package of two groups is installed once: {list}");
        // a package this release does not have is skipped, not fatal
        assert!(script.contains("apt-cache show"));
        let c = install_tools_command("Ubuntu", &groups);
        let a = args(&c);
        assert!(a.windows(2).any(|w| w == ["-u", "root"]));
        assert_eq!(a.last().unwrap(), &script);
        // only the essentials: no compilers
        let light = install_tools_script(&essential_groups());
        assert!(light.contains("curl") && !light.contains("build-essential"));
    }

    #[test]
    fn a_group_can_be_cut_down_to_the_packages_that_are_chosen() {
        let g = ToolGroup::new("Files and text", &["zip", "jq", "vim"], "zip unzip jq vim vim");
        assert_eq!(g.package_list(), ["zip", "unzip", "jq", "vim"], "each package once");
        let cut = g.with_packages(|p| p != "vim" && p != "zip");
        assert_eq!(cut.packages, "unzip jq");
        assert_eq!(cut.programs, g.programs, "what is looked for stays");
        assert_eq!(cut.name, g.name);
        assert_eq!(g.with_packages(|_| false).packages, "");
        // what is left out of the groups is left out of the script
        let script = install_tools_script(&[cut, ToolGroup::new("Debugging", &["gdb"], "gdb valgrind")]);
        let list = &script[script.find("for p in ").unwrap()..script.find("; do").unwrap()];
        assert_eq!(list, "for p in unzip jq gdb valgrind");
    }

    #[test]
    fn words_of_a_custom_tool_are_safe_to_put_in_a_script() {
        for ok in ["libssl-dev", "g++", "python3.12", "libc6:i386", "x", "7zip", "nodejs_x"] {
            assert!(is_safe_word(ok), "{ok}");
        }
        for bad in ["", "-rf", ".hidden", "a;b", "a b", "$(x)", "a`b`", "a|b", "a&b", "a'b", "a\"b", "a\nb", "a>b", "*", "a/b"] {
            assert!(!is_safe_word(bad), "{bad:?}");
        }
        let g = custom_group("  Libraries ", "libssl-dev  zlib1g-dev", "").unwrap();
        assert_eq!((g.name.as_str(), g.packages.as_str(), g.custom), ("Libraries", "libssl-dev zlib1g-dev", true));
        assert_eq!(g.programs, ["dpkg:libssl-dev", "dpkg:zlib1g-dev"], "without programs the packages are looked for");
        let g = custom_group("Net", "tcpdump nmap", "tcpdump  nmap").unwrap();
        assert_eq!(g.programs, ["tcpdump", "nmap"]);
        assert!(custom_group("", "nmap", "").is_none(), "a name is needed");
        assert!(custom_group("x", "  ", "").is_none(), "and a package");
        assert!(custom_group("x", "nmap; rm -rf /", "").is_none());
        assert!(custom_group("x", "nmap", "$(reboot)").is_none());
        assert_eq!(check_label("dpkg:libssl-dev"), "libssl-dev");
        assert_eq!(check_label("make"), "make");
    }

    #[test]
    fn a_package_without_a_program_is_asked_about_through_dpkg() {
        let mut groups = module_groups();
        groups.push(custom_group("Libraries", "libssl-dev", "").unwrap());
        groups.push(custom_group("Net", "tcpdump", "tcpdump").unwrap());
        let check = tools_check_script(&groups);
        assert!(check.contains("echo \"dpkg:libssl-dev=$(dpkg-query -W -f='${Status}' libssl-dev 2>/dev/null | grep 'ok installed')\""), "{check}");
        assert!(check.contains("echo \"tcpdump=$(command -v tcpdump 2>/dev/null)\""));
        let text = "make=/usr/bin/make\ndpkg:libssl-dev=install ok installed\ntcpdump=\n";
        let found = parse_tools(text, &groups);
        assert!(found.contains(&("dpkg:libssl-dev".to_string(), true)));
        assert!(found.contains(&("tcpdump".to_string(), false)));
        assert!(missing_core(&found).iter().all(|m| BUILD_TOOLS.contains(&m.as_str())), "a custom tool never makes the banner");
    }

    #[test]
    fn the_tools_the_user_added_are_installed_with_every_package_manager() {
        let libs = custom_group("Libraries", "libssl-dev zlib1g-dev", "").unwrap();
        let mut groups = module_groups();
        groups.push(libs.clone());
        let script = install_tools_script(&groups);
        let list = &script[script.find("for p in ").unwrap()..script.find("; do").unwrap()];
        assert!(list.contains("libssl-dev zlib1g-dev"), "apt: {list}");
        // the others: the core set, then the packages one by one, a failing one is skipped
        assert!(script.contains("dnf install -y make gcc"));
        assert!(script.contains("for p in libssl-dev zlib1g-dev; do dnf install -y \"$p\" || echo"));
        assert!(script.contains("for p in libssl-dev zlib1g-dev; do pacman -S --noconfirm \"$p\" || echo"));
        assert!(script.contains("for p in libssl-dev zlib1g-dev; do apk add \"$p\" || echo"));
        assert!(script.contains("for p in libssl-dev zlib1g-dev; do zypper --non-interactive install \"$p\" || echo"));
        // only the user's own tools: the core set is not brought along
        let only = install_tools_script(&[libs]);
        assert!(!only.contains("dnf install -y make") && !only.contains("base-devel") && only.contains("for p in libssl-dev zlib1g-dev; do dnf install"), "{only}");
        // nothing of one's own: no loop of one's own
        assert!(!install_tools_script(&module_groups()).contains("could not be installed with"));
    }

    #[test]
    fn the_install_script_does_not_fail_for_the_tools_it_left_out() {
        // the check at the end only reports what is there (the script runs with `set -e`)
        let s = install_tools_script(&[ToolGroup::new("Debugging", &["gdb"], "gdb")]);
        let verify = &s[s.find("@@light stage verify").unwrap()..];
        assert!(verify.contains("if command -v \"$t\""), "{verify}");
        assert!(!verify.contains("\nmake --version") && !verify.contains("\ngcc --version"));
        // and with nothing chosen from apt nothing is asked of apt-get
        assert!(s.contains("if [ -n \"$have\" ]; then apt-get install -y $have; fi"));
    }

    #[test]
    fn git_setup_quotes_everything() {
        let s = git_setup_script("O'Brien", "me@example.com", Some("/mnt/c/Program Files/Git/mingw64/bin/git-credential-manager.exe"));
        assert!(s.contains(r"git config --global user.name 'O'\''Brien'"));
        assert!(s.contains("user.email 'me@example.com'"));
        assert!(s.contains("credential.helper '!") && s.contains("/mnt/c/Program Files/Git/mingw64/bin/git-credential-manager.exe"));
        assert!(!git_setup_script("", "", None).contains("--global user.name '"));
    }

    #[test]
    fn windows_paths_become_linux_paths() {
        let t = t();
        assert_eq!(to_linux(Path::new(r"C:\Users\me\proj\a.lisp"), &t), "/mnt/c/Users/me/proj/a.lisp");
        assert_eq!(to_linux(Path::new("D:/x"), &t), "/mnt/d/x");
        assert_eq!(to_linux(Path::new(r"C:\"), &t), "/mnt/c");
        assert_eq!(to_linux(Path::new(r"\\?\C:\long\path"), &t), "/mnt/c/long/path");
        assert_eq!(to_linux(Path::new(r"\\wsl.localhost\Ubuntu-24.04\home\me\p"), &t), "/home/me/p");
        assert_eq!(to_linux(Path::new(r"\\wsl$\Ubuntu\home\me"), &t), "/home/me");
        assert_eq!(to_linux(Path::new(r"\\WSL.LOCALHOST\Ubuntu"), &t), "/");
        assert_eq!(to_linux(Path::new(r"src\a.lisp"), &t), "src/a.lisp");
        assert_eq!(to_linux(Path::new("/already/linux"), &t), "/already/linux");
        let custom = Target::new("Debian", "/win");
        assert_eq!(custom.mount_root, "/win/");
        assert_eq!(to_linux(Path::new(r"E:\data"), &custom), "/win/e/data");
    }

    #[test]
    fn linux_paths_come_back_to_windows() {
        let t = t();
        assert_eq!(to_windows("/mnt/c/Users/me/a.lisp", &t), PathBuf::from(r"C:\Users\me\a.lisp"));
        assert_eq!(to_windows("/mnt/d", &t), PathBuf::from(r"D:\"));
        assert_eq!(to_windows("/home/me/p/x.lisp", &t), PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\p\x.lisp"));
        assert_eq!(to_windows("/mnt/wsl/shared", &t), PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\mnt\wsl\shared"));
        assert_eq!(to_windows("relative/x", &t), PathBuf::from("relative/x"));
        for p in [r"C:\Users\me\proj\a.lisp", r"\\wsl.localhost\Ubuntu-24.04\home\me\x"] {
            assert_eq!(to_windows(&to_linux(Path::new(p), &t), &t), PathBuf::from(p), "round trip of {p}");
        }
    }

    #[test]
    fn decodes_utf8_and_utf16_output() {
        assert_eq!(decode("Версия WSL: 2.6.1.0\r\n".as_bytes()), "Версия WSL: 2.6.1.0\r\n");
        let utf16: Vec<u8> = "\u{feff}NAME  STATE\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode(&utf16), "NAME  STATE\r\n");
        assert_eq!(decode(b""), "");
    }

    #[test]
    fn parses_the_wsl_version_in_any_language() {
        let ru = "Версия WSL: 2.6.1.0\r\nВерсия ядра: 6.6.87.2-1\r\nВерсия WSLg: 1.0.66\r\n";
        assert_eq!(parse_version(ru).as_deref(), Some("2.6.1.0"));
        assert_eq!(parse_version("WSL version: 2.4.13.0\nKernel version: 5.15.167.4-1").as_deref(), Some("2.4.13.0"));
        assert_eq!(parse_version("Usage: wsl.exe [Argument]"), None);
    }

    #[test]
    fn parses_installed_distributions_and_ignores_messages() {
        let text = "  NAME            STATE           VERSION\r\n* Ubuntu-24.04    Stopped         2\r\n  Debian          Running         2\r\n  Old             Остановлено     1\r\n";
        let d = parse_list_verbose(text);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0], Distro { name: "Ubuntu-24.04".into(), state: "Stopped".into(), version: 2, default: true });
        assert!(!d[1].default && d[1].state == "Running");
        assert_eq!(d[2].version, 1);
        let none = "подсистема Windows для Linux не имеет установленных дистрибутивов.\r\nЧтобы устранить эту проблему, установите дистрибутив с инструкциями ниже:\r\n\r\nИспользуйте \"wsl.exe --list --online' для перечисления доступных дистрибутивов\r\nи \"wsl.exe --install <Distro>\" для установки.\r\n";
        assert!(parse_list_verbose(none).is_empty());
        let probe = Probe { presence: Presence::Installed { version: None }, distros: d };
        assert_eq!(probe.default_distro().unwrap().name, "Ubuntu-24.04");
    }

    #[test]
    fn parses_the_online_catalog_after_its_header() {
        let text = "Ниже указан список допустимых дистрибутивов.\r\nУстановить с помощью \"wsl.exe --install <Distro>\".\r\n\r\nNAME                            FRIENDLY NAME\r\nDebian                          Debian GNU/Linux\r\nUbuntu-24.04                    Ubuntu 24.04 LTS\r\narchlinux                       Arch Linux\r\n";
        let l = parse_online(text);
        assert_eq!(l.len(), 3);
        assert_eq!(l[1], OnlineDistro { name: "Ubuntu-24.04".into(), friendly: "Ubuntu 24.04 LTS".into() });
        assert!(parse_online("no network").is_empty());
    }

    #[test]
    fn sbcl_check_reports_path_version_and_mount_root() {
        let c = parse_sbcl_check("LIGHT_SBCL=/usr/bin/sbcl\nLIGHT_VERSION=SBCL 2.2.9.debian\nLIGHT_MOUNT=/mnt/c/\n");
        assert_eq!(c, SbclCheck { path: Some("/usr/bin/sbcl".into()), version: Some("SBCL 2.2.9.debian".into()), mount_root: Some("/mnt/".into()) });
        let missing = parse_sbcl_check("LIGHT_SBCL=\nLIGHT_MOUNT=/win/c/\n");
        assert_eq!(missing, SbclCheck { path: None, version: None, mount_root: Some("/win/".into()) });
        assert!(sbcl_check_script("my sbcl").contains("command -v 'my sbcl'"));
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn image_files_are_told_apart() {
        assert_eq!(image_kind(Path::new(r"C:\Downloads\ubuntu-24.04.wsl")), ImageKind::Wsl);
        assert_eq!(image_kind(Path::new("rootfs.tar.gz")), ImageKind::Tar);
        assert_eq!(image_kind(Path::new("ubuntu-24.04.3-desktop-amd64.iso")), ImageKind::Iso);
        assert_eq!(image_kind(Path::new("notes.txt")), ImageKind::Unknown);
        assert!(install_file_command(Path::new("x.iso"), "U", Path::new(r"C:\w")).is_err());
        let c = install_file_command(Path::new(r"C:\d\u.wsl"), "U", Path::new(r"C:\w")).unwrap();
        assert!(args(&c).ends_with(&["--install", "--no-launch", "--from-file", r"C:\d\u.wsl", "--name", "U", "--location", r"C:\w"].map(String::from)));
        let c = install_file_command(Path::new(r"C:\d\root.tar"), "U", Path::new(r"C:\w")).unwrap();
        assert!(args(&c).ends_with(&["--import".into(), "U".into(), r"C:\w".into(), r"C:\d\root.tar".into()]));
    }

    #[test]
    fn commands_run_without_a_shell_in_the_translated_directory() {
        let t = t();
        let c = exec(&t, Some(Path::new(r"C:\proj")), "/usr/bin/sbcl", &["--eval".into(), "(format t \"~a\" 1)".into()]);
        let a = args(&c);
        let at = a.iter().position(|x| x == "-d").unwrap();
        assert_eq!(a[at..], ["-d", "Ubuntu-24.04", "--cd", "/mnt/c/proj", "-e", "/usr/bin/sbcl", "--eval", "(format t \"~a\" 1)"]);
        let s = shell(&t, Path::new(r"C:\proj"), "ls -la | wc -l", &[("A".into(), "1".into())], "light-1-2x");
        assert!(args(&s).ends_with(&["-e".into(), "env".into(), "A=1".into(), "sh".into(), "-lc".into(), "ls -la | wc -l".into(), "light-1-2x".into()]));
        let i = install_distro_command("Ubuntu-24.04", Some(Path::new(r"C:\WSL\Ubuntu")));
        assert!(args(&i).ends_with(&["--install".into(), "-d".into(), "Ubuntu-24.04".into(), "--no-launch".into(), "--web-download".into(), "--location".into(), r"C:\WSL\Ubuntu".into()]));
        assert!(args(&install_sbcl_command("Debian")).ends_with(&["-u".into(), "root".into(), "-e".into(), "sh".into(), "-c".into(), INSTALL_SBCL_SCRIPT.into()]));
    }

    #[test]
    fn markers_are_unique_and_the_kill_pattern_does_not_match_itself() {
        let (a, b) = (new_marker(), new_marker());
        assert_ne!(a, b);
        assert!(a.ends_with('x'));
        let p = marker_pattern("light-12-3x");
        assert_eq!(p, "[l]ight-12-3x");
        assert!(!p.contains("light-12-3x"), "the cleanup's own command line must not match");
    }

    #[test]
    fn install_scripts_announce_their_phases() {
        use crate::setup_progress::MARKER;
        let tools = install_tools_script(&module_groups());
        for script in [INSTALL_SBCL_SCRIPT, tools.as_str()] {
            let marker = |name: &str| format!("echo '{MARKER}{name}'");
            // the lists phase comes first, the check last, and every package manager announces its install phase
            let prepare = script.find(&marker("prepare")).expect("prepare");
            let verify = script.find(&marker("verify")).expect("verify");
            assert!(prepare < script.find("if command -v apt-get").unwrap());
            assert!(verify > script.rfind("apk add").unwrap());
            for manager in ["apt-get install", "dnf install", "zypper --non-interactive install", "pacman -Sy", "apk add"] {
                let at = script.find(manager).unwrap_or_else(|| panic!("{manager}"));
                let announced = script[..at].rfind(&marker("install")).expect("install marker");
                // nothing that installs or ends the phase lies between the marker and the command (apt only picks its list)
                let between = &script[announced..at];
                assert!(!between.contains("install -y") && !between.contains("apk add") && !between.contains(&marker("verify")), "{manager}: the install marker is right before the command");
            }
        }
    }

    #[test]
    fn the_localhost_proxy_notice_is_dropped() {
        let ru = "wsl: Конфигурация прокси-сервера localhost обнаружена, но не отражена в WSL. WSL в режиме NAT не поддерживает прокси-серверы localhost.\n";
        let en = "wsl: A localhost proxy configuration was detected but not mirrored into WSL. WSL in NAT mode does not support localhost proxies.\r\n";
        assert_eq!(strip_notices(&format!("{ru}Doxyfile\nREADME.md\n")), "Doxyfile\nREADME.md\n");
        assert_eq!(strip_notices(&format!("a\n{en}b\n")), "a\nb\n");
        assert_eq!(strip_notices(ru), "");
        // everything else stays, including other messages of wsl.exe and a partial line
        let keep = "wsl: Failed to start the distribution\nmake: *** [all] Error 2\nno newline";
        assert_eq!(strip_notices(keep), keep);
        assert_eq!(strip_notices("localhost NAT but not from wsl\n"), "localhost NAT but not from wsl\n");
    }

    /// Needs a real WSL distribution (`LIGHT_TEST_DISTRO`, default Ubuntu-24.04): run with
    /// `cargo test -p lisp-runtime -- --ignored real_wsl`. Starts `sh -> sh -> sleep` the way the terminal starts a
    /// command that starts a program (`make run` -> qemu), with the program immune to hangup, and checks that
    /// `kill_marked` leaves nothing behind.
    #[test]
    #[ignore = "needs a real WSL distribution"]
    fn real_wsl_kill_marked_ends_grandchildren() {
        let distro = std::env::var("LIGHT_TEST_DISTRO").or_else(|_| std::env::var("AIR_TEST_DISTRO")).unwrap_or_else(|_| "Ubuntu-24.04".into());
        let t = Target::new(&distro, "");
        let marker = new_marker();
        // a number no other process of the machine runs `sleep` with
        let secs = 4000 + std::process::id() % 1000;
        // the inner shell ignores the hangup the kernel sends when the session ends, like nohup or a GUI program does
        let cmd = shell(&t, Path::new("C:\\"), &format!("sh -c 'trap \"\" HUP; sleep {secs}; true'"), &[], &marker);
        let handle = crate::runner::spawn_launch(crate::discover::Launch { cmd, cleanup: Some(Cleanup { target: t.clone(), marker: marker.clone() }) }).expect("start");
        // `[s]leep`: the pgrep that looks for it does not find itself
        let sleeping = || {
            let c = exec(&t, None, "sh", &["-c".into(), format!("pgrep -f '[s]leep {secs}' | wc -l")]);
            let out = capture(c, Duration::from_secs(30)).expect("pgrep").text;
            out.lines().rev().find_map(|l| l.trim().parse::<u32>().ok()).unwrap_or(0)
        };
        for _ in 0..20 {
            if sleeping() > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(sleeping() >= 1, "the command did not start");
        // the Windows side (wsl.exe) stays alive here: this is about what happens inside Linux
        kill_marked(&t, &marker);
        std::thread::sleep(Duration::from_secs(4));
        let left = sleeping();
        handle.kill();
        assert_eq!(left, 0, "sleep outlived kill_marked");
    }

    /// Needs a real Debian/Ubuntu WSL distribution (`LIGHT_TEST_DISTRO`, default Ubuntu-24.04): run the whole install
    /// script with `apt-get install` turned into a simulation, so nothing is installed, and see that it gets through
    /// (package names exist, the lists phase and the check run) and that the progress model follows its output.
    #[test]
    #[ignore = "needs a real WSL distribution"]
    fn real_wsl_install_script_runs_in_simulation() {
        use crate::setup_progress::{Kind, Stage, Tracker};
        let distro = std::env::var("LIGHT_TEST_DISTRO").or_else(|_| std::env::var("AIR_TEST_DISTRO")).unwrap_or_else(|_| "Ubuntu-24.04".into());
        // a wrapper first on PATH: `apt-get install ...` becomes `apt-get -s install ...` (a simulation)
        let shim = r#"d=$(mktemp -d)
cat > $d/apt-get <<'EOS'
#!/bin/sh
if [ "$1" = install ]; then exec /usr/bin/apt-get -s "$@"; else exec /usr/bin/apt-get "$@"; fi
EOS
chmod +x $d/apt-get
PATH=$d:$PATH
export PATH
"#;
        let mut c = wsl_command();
        c.args(["-d", &distro, "-u", "root", "-e", "sh", "-c", &format!("{shim}{}", install_tools_script(&module_groups()))]);
        let out = capture(c, Duration::from_secs(600)).expect("run");
        assert!(out.ok(), "the script failed: {}", out.text);
        assert!(!out.text.contains("Skipped"), "a package of the list does not exist in {distro}: {}", out.text.lines().filter(|l| l.contains("Skipped")).collect::<Vec<_>>().join("
"));
        for pkg in ["cmake", "clangd", "valgrind", "python3-pip", "nasm"] {
            assert!(out.text.contains(&format!("Inst {pkg} ")) || out.text.contains(&format!("Inst {pkg}:")) || out.text.contains(&format!("{pkg} is already")), "{pkg} is not part of the plan: {}", out.text);
        }
        let mut t = Tracker::new(Kind::Packages);
        t.feed(&out.text);
        assert_eq!(t.progress().stage, Stage::Verify, "all phases were announced");
    }
}

//! How far a Linux setup step has got, read from the output of the installer.
//!
//! `wsl --install` prints a percentage while it downloads. Package managers print what they unpack and configure;
//! the scripts of [`crate::wsl`] also print `@@light stage NAME` between their phases. A [`Tracker`] turns this
//! stream into a fraction (when the total is known) and a stage, so the UI can draw a real progress bar instead of
//! a spinner. Nothing here starts a process: the tracker only reads text.

/// Prefix of the lines the setup scripts print between their phases (the UI hides them from the log).
pub const MARKER: &str = "@@light stage ";

/// What kind of setup step the output belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `wsl --install --no-distribution` (nothing is printed while Windows asks for administrator rights).
    Wsl,
    /// `wsl --install -d NAME`: a download with percentages, then the registration.
    Distro,
    /// `wsl --import` / `--from-file`: no output until it is done.
    Import,
    /// A script of the package manager: SBCL, the build tools.
    Packages,
    /// Short configuration commands.
    Config,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Starting,
    InstallWsl,
    Download,
    Register,
    Import,
    Prepare,
    Install,
    Verify,
    Config,
}

impl Stage {
    /// English text of the stage (the UI translates it).
    pub fn label(self) -> &'static str {
        match self {
            Stage::Starting => "Starting...",
            Stage::InstallWsl => "Installing WSL (confirm the administrator prompt if Windows asks)",
            Stage::Download => "Downloading the distribution",
            Stage::Register => "Installing the distribution",
            Stage::Import => "Importing the image (this can take a few minutes)",
            Stage::Prepare => "Updating the package lists",
            Stage::Install => "Downloading and installing packages",
            Stage::Verify => "Checking the result",
            Stage::Config => "Configuring",
        }
    }

    fn from_marker(name: &str) -> Option<Stage> {
        match name.trim() {
            "prepare" => Some(Stage::Prepare),
            "install" => Some(Stage::Install),
            "verify" => Some(Stage::Verify),
            _ => None,
        }
    }
}

/// What the UI draws: a stage and, when the total is known, how much is done (0.0..=1.0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub stage: Stage,
    pub fraction: Option<f32>,
}

/// Where the package phase sits in the whole step (the rest is the lists before it and the check after it).
const PREPARE_FROM: f32 = 0.03;
const INSTALL_FROM: f32 = 0.20;
const VERIFY_AT: f32 = 0.96;

#[derive(Debug)]
pub struct Tracker {
    kind: Kind,
    stage: Stage,
    fraction: Option<f32>,
    /// Text of the line being written (a progress bar redraws it with `\r`).
    carry: String,
    /// Lines of the package lists phase seen so far (the phase has no total).
    prepare_lines: u32,
    /// apt: how many packages are being installed, and how many were fetched / unpacked / set up.
    apt_total: u32,
    apt: [u32; 3],
    /// dnf: the download half and the install half of the numbered lines.
    download_part: f32,
    install_part: f32,
}

impl Tracker {
    pub fn new(kind: Kind) -> Tracker {
        let stage = match kind {
            Kind::Wsl => Stage::InstallWsl,
            Kind::Distro => Stage::Download,
            Kind::Import => Stage::Import,
            Kind::Packages => Stage::Starting,
            Kind::Config => Stage::Config,
        };
        Tracker { kind, stage, fraction: None, carry: String::new(), prepare_lines: 0, apt_total: 0, apt: [0; 3], download_part: 0.0, install_part: 0.0 }
    }

    pub fn progress(&self) -> Progress {
        Progress { stage: self.stage, fraction: self.fraction }
    }

    /// The step ended: a successful one is complete, whatever it printed.
    pub fn finish(&mut self, ok: bool) {
        if ok {
            self.fraction = Some(1.0);
        }
    }

    /// Feed a chunk of output (any size; lines may be split between chunks). Returns true when the progress changed.
    pub fn feed(&mut self, text: &str) -> bool {
        let before = (self.stage, self.fraction);
        for ch in text.chars() {
            match ch {
                '\n' | '\r' => {
                    let line = std::mem::take(&mut self.carry);
                    self.line(&line, true);
                }
                c => self.carry.push(c),
            }
        }
        // a progress bar is redrawn in place and has no line end until it is done: its percentage counts now
        if !self.carry.is_empty() {
            let partial = self.carry.clone();
            self.line(&partial, false);
        }
        before != (self.stage, self.fraction)
    }

    /// `complete`: the line has ended, so counting lines (packages) may count it; a partial one only shows a percentage.
    fn line(&mut self, line: &str, complete: bool) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if let Some(name) = line.strip_prefix(MARKER) {
            if complete && let Some(s) = Stage::from_marker(name) {
                self.enter(s);
            }
            return;
        }
        match self.kind {
            Kind::Wsl => {
                if let Some(p) = percent(line) {
                    self.set(p / 100.0);
                }
            }
            Kind::Distro => self.distro_line(line),
            Kind::Packages if complete => self.package_line(line),
            _ => {}
        }
    }

    /// Raise the fraction; it never goes back (the output of a package manager is not monotonic by itself).
    fn set(&mut self, f: f32) {
        let f = f.clamp(0.0, 1.0);
        if self.fraction.is_none_or(|old| f > old) {
            self.fraction = Some(f);
        }
    }

    fn enter(&mut self, s: Stage) {
        self.stage = s;
        match s {
            Stage::Prepare => self.set(PREPARE_FROM),
            Stage::Install => self.set(INSTALL_FROM),
            Stage::Verify => self.set(VERIFY_AT),
            _ => {}
        }
    }

    fn distro_line(&mut self, line: &str) {
        // `Downloading: Ubuntu`, a bar with a percentage, `Installing: Ubuntu`, `Distribution successfully installed`
        const DOWNLOAD: f32 = 0.9;
        if self.stage == Stage::Download
            && let Some(p) = percent(line)
        {
            self.set(p / 100.0 * DOWNLOAD);
        }
        let lower = line.to_lowercase();
        if lower.starts_with("installing") && percent(line).is_none() {
            self.stage = Stage::Register;
            self.set(DOWNLOAD + 0.02);
        } else if lower.contains("successfully") {
            self.set(0.99);
        }
    }

    fn package_line(&mut self, line: &str) {
        match self.stage {
            Stage::Prepare => {
                if ["Hit:", "Get:", "Ign:", "Fetched", "Reading package lists"].iter().any(|p| line.starts_with(p)) {
                    self.prepare_lines += 1;
                    let n = self.prepare_lines as f32;
                    self.set(PREPARE_FROM + (INSTALL_FROM - PREPARE_FROM) * 0.9 * n / (n + 8.0));
                }
            }
            Stage::Install => self.install_line(line),
            _ => {}
        }
    }

    fn install_line(&mut self, line: &str) {
        // apt: "0 upgraded, 12 newly installed, 0 to remove ...", then Get: / Unpacking / Setting up per package
        if let Some(n) = apt_new_packages(line) {
            self.apt_total = n;
            return;
        }
        if self.apt_total > 0 {
            let which = if line.starts_with("Get:") {
                Some(0)
            } else if line.starts_with("Unpacking ") {
                Some(1)
            } else if line.starts_with("Setting up ") {
                Some(2)
            } else {
                None
            };
            if let Some(i) = which {
                if i == 1 {
                    // packages that were already downloaded print no Get: line
                    self.apt[0] = self.apt[0].max(self.apt_total);
                }
                self.apt[i] = (self.apt[i] + 1).min(self.apt_total);
                let done: u32 = self.apt.iter().sum();
                self.set(INSTALL_FROM + (VERIFY_AT - INSTALL_FROM) * done as f32 / (3 * self.apt_total) as f32);
                return;
            }
        }
        // dnf / zypper / pacman / apk: numbered lines `(3/12) Installing x`, `[3/12] Installing x`, `Installing : x 3/12`
        if let Some((i, n)) = counted(line) {
            let part = i as f32 / n as f32;
            if line.starts_with('(') && line.contains("):") && !line.to_lowercase().contains("install") {
                // dnf: "(3/12): package.rpm  45% | 1.2 MB/s" - the download half
                self.download_part = self.download_part.max(part);
            } else if line.to_lowercase().contains("install") {
                self.install_part = self.install_part.max(part);
            } else {
                return;
            }
            let x = if self.download_part > 0.0 { 0.5 * self.download_part + 0.5 * self.install_part } else { self.install_part };
            self.set(INSTALL_FROM + (VERIFY_AT - INSTALL_FROM) * x);
        }
    }
}

/// `N newly installed` of apt's summary line (also "1 newly installed").
fn apt_new_packages(line: &str) -> Option<u32> {
    let at = line.find(" newly installed")?;
    let head = &line[..at];
    let start = head.rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    head[start..].parse::<u32>().ok().filter(|n| *n > 0)
}

/// `(i/n)`, `[i/n]` at the start of the line or `i/n` at its end -> (i, n), when 1 <= i <= n.
fn counted(line: &str) -> Option<(u32, u32)> {
    let pair = |s: &str| -> Option<(u32, u32)> {
        let (a, b) = s.split_once('/')?;
        let (i, n) = (a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?);
        (1 <= i && i <= n).then_some((i, n))
    };
    for (open, close) in [('(', ')'), ('[', ']')] {
        if let Some(rest) = line.strip_prefix(open)
            && let Some(end) = rest.find(close)
            && let Some(p) = pair(&rest[..end])
        {
            return Some(p);
        }
    }
    // dnf 4: "Installing       : gcc-13.2.1-6.fc40.x86_64            12/60"
    let last = line.split_whitespace().last()?;
    pair(last)
}

/// The last percentage printed on `line` (`[=====  45.5%   ]` -> 45.5).
pub fn percent(line: &str) -> Option<f32> {
    let at = line.rfind('%')?;
    let head = &line[..at];
    let start = head.rfind(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',')).map_or(0, |i| i + 1);
    head[start..].replace(',', ".").parse::<f32>().ok().filter(|p| (0.0..=100.0).contains(p))
}

/// `1:05` / `12:30` for a number of seconds.
pub fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kind: Kind, text: &str) -> Tracker {
        let mut t = Tracker::new(kind);
        t.feed(text);
        t
    }

    fn frac(t: &Tracker) -> f32 {
        t.progress().fraction.expect("a fraction")
    }

    #[test]
    fn percentages() {
        assert_eq!(percent("[====  42.5%  ]"), Some(42.5));
        assert_eq!(percent("Загрузка 7,5%"), Some(7.5));
        assert_eq!(percent("no number %"), None);
        assert_eq!(percent("no percent"), None);
        assert_eq!(percent("150%"), None);
        assert_eq!(clock(65), "1:05");
        assert_eq!(clock(0), "0:00");
    }

    #[test]
    fn wsl_download_then_registration() {
        let mut t = Tracker::new(Kind::Distro);
        assert_eq!(t.progress(), Progress { stage: Stage::Download, fraction: None });
        t.feed("Downloading: Ubuntu 24.04 LTS\n");
        assert_eq!(t.progress().fraction, None, "no number yet: the bar stays indeterminate");
        t.feed("\r[=====               ] 25.0%");
        let a = frac(&t);
        t.feed("\r[==========          ] 50.0%");
        let b = frac(&t);
        assert!(a > 0.2 && a < 0.25 && b > 0.44 && b < 0.46, "{a} {b}");
        t.feed("\r[====================] 100.0%\nInstalling: Ubuntu 24.04 LTS\n");
        assert_eq!(t.progress().stage, Stage::Register);
        assert!(frac(&t) > 0.9 && frac(&t) < 1.0);
        t.feed("Distribution successfully installed. It can be launched via wsl.exe -d Ubuntu-24.04\n");
        assert!(frac(&t) >= 0.99);
        t.finish(true);
        assert_eq!(frac(&t), 1.0);
    }

    #[test]
    fn chunks_may_split_lines() {
        let mut t = Tracker::new(Kind::Distro);
        t.feed("\r[=====      ");
        t.feed("         ] 4");
        t.feed("0.0%");
        assert!((frac(&t) - 0.36).abs() < 0.001, "{}", frac(&t));
    }

    #[test]
    fn wsl_itself_and_silent_steps() {
        let t = run(Kind::Wsl, "");
        assert_eq!(t.progress(), Progress { stage: Stage::InstallWsl, fraction: None });
        let t = run(Kind::Wsl, "Installing: Windows Subsystem for Linux\n\r[==========          ] 50.0%");
        assert_eq!(frac(&t), 0.5);
        let t = run(Kind::Import, "whatever it prints\n");
        assert_eq!(t.progress(), Progress { stage: Stage::Import, fraction: None });
        let mut t = run(Kind::Config, "name: me\n");
        assert_eq!(t.progress().fraction, None);
        t.finish(false);
        assert_eq!(t.progress().fraction, None, "a failed step is not complete");
    }

    #[test]
    fn apt_counts_packages() {
        let mut t = Tracker::new(Kind::Packages);
        t.feed(&format!("{MARKER}prepare\n"));
        assert_eq!(t.progress().stage, Stage::Prepare);
        let start = frac(&t);
        for n in 0..24 {
            t.feed(&format!("Get:{n} http://archive.ubuntu.com/ubuntu noble InRelease [126 kB]\n"));
        }
        let lists = frac(&t);
        assert!(lists > start && lists < INSTALL_FROM, "{lists}");
        t.feed("Reading package lists... Done\n");
        t.feed(&format!("{MARKER}install\n"));
        assert_eq!(t.progress().stage, Stage::Install);
        assert_eq!(frac(&t), INSTALL_FROM);
        t.feed("Reading package lists...\nThe following NEW packages will be installed:\n  sbcl libffi\n0 upgraded, 2 newly installed, 0 to remove and 4 not upgraded.\nNeed to get 9 MB of archives.\n");
        assert_eq!(frac(&t), INSTALL_FROM, "the total is known, nothing is done yet");
        let mut last = frac(&t);
        for line in ["Get:1 http://archive.ubuntu.com/ubuntu noble/main libffi8 amd64 [20 kB]", "Get:2 http://archive.ubuntu.com/ubuntu noble/universe sbcl amd64 [9 MB]", "Selecting previously unselected package libffi8.", "Unpacking libffi8:amd64 (3.4.6) ...", "Unpacking sbcl (2:2.2.9) ...", "Setting up libffi8:amd64 (3.4.6) ...", "Setting up sbcl (2:2.2.9) ..."] {
            t.feed(&format!("{line}\n"));
            assert!(frac(&t) >= last, "never goes back: {line}");
            last = frac(&t);
        }
        assert!((last - VERIFY_AT).abs() < 0.001, "all packages set up: {last}");
        t.feed(&format!("{MARKER}verify\nSBCL 2.2.9.debian\n"));
        assert_eq!(t.progress().stage, Stage::Verify);
        t.finish(true);
        assert_eq!(frac(&t), 1.0);
    }

    #[test]
    fn apt_with_everything_already_downloaded() {
        let mut t = Tracker::new(Kind::Packages);
        t.feed(&format!("{MARKER}install\n0 upgraded, 4 newly installed, 0 to remove and 0 not upgraded.\nUnpacking a (1) ...\n"));
        // no Get: lines: the downloads count as done, so the bar does not stall at two thirds
        let f = (frac(&t) - INSTALL_FROM) / (VERIFY_AT - INSTALL_FROM);
        assert!((f - 5.0 / 12.0).abs() < 0.001, "{f}");
    }

    #[test]
    fn dnf_zypper_pacman_apk_numbered_lines() {
        // dnf 4: download half, then the install half
        let mut t = Tracker::new(Kind::Packages);
        t.feed(&format!("{MARKER}install\n"));
        t.feed("(1/4): make-4.4.1-5.fc40.x86_64.rpm   1.2 MB/s | 590 kB  00:00\n");
        t.feed("(4/4): gcc-14.1.1-1.fc40.x86_64.rpm   9.9 MB/s |  38 MB   00:03\n");
        let downloaded = frac(&t);
        assert!((downloaded - (INSTALL_FROM + (VERIFY_AT - INSTALL_FROM) * 0.5)).abs() < 0.001, "{downloaded}");
        t.feed("  Installing       : make-1:4.4.1-5.fc40.x86_64                  1/4\n");
        assert!(frac(&t) > downloaded);
        t.feed("  Installing       : gcc-14.1.1-1.fc40.x86_64                    4/4\n");
        assert!((frac(&t) - VERIFY_AT).abs() < 0.001);
        // zypper
        let t = run_install("(3/6) Installing: gcc-13-1.x86_64 [done]\n");
        assert!((frac(&t) - (INSTALL_FROM + (VERIFY_AT - INSTALL_FROM) * 0.5)).abs() < 0.001);
        // pacman: other numbered phases (keys, integrity) are not installs
        let t = run_install("(1/3) checking keys in keyring\n(3/3) checking package integrity\n");
        assert_eq!(frac(&t), INSTALL_FROM);
        let t = run_install("(2/8) installing glibc\n");
        assert!((frac(&t) - (INSTALL_FROM + (VERIFY_AT - INSTALL_FROM) * 0.25)).abs() < 0.001);
        // apk
        let t = run_install("(5/10) Installing build-base (0.5-r3)\n");
        assert!((frac(&t) - (INSTALL_FROM + (VERIFY_AT - INSTALL_FROM) * 0.5)).abs() < 0.001);
        // dnf 5
        let t = run_install("[ 3/6] Installing make\n");
        assert!((frac(&t) - (INSTALL_FROM + (VERIFY_AT - INSTALL_FROM) * 0.5)).abs() < 0.001);
    }

    fn run_install(text: &str) -> Tracker {
        let mut t = Tracker::new(Kind::Packages);
        t.feed(&format!("{MARKER}install\n{text}"));
        t
    }

    #[test]
    fn lines_outside_a_stage_and_markers_are_ignored() {
        // nothing counts before the script says which phase it is in
        let mut t = Tracker::new(Kind::Packages);
        t.feed("0 upgraded, 3 newly installed\nUnpacking x\n");
        assert_eq!(t.progress(), Progress { stage: Stage::Starting, fraction: None });
        // an unfinished marker line is not acted on until it is complete
        t.feed("@@light stage insta");
        assert_eq!(t.progress().stage, Stage::Starting);
        t.feed("ll\n");
        assert_eq!(t.progress().stage, Stage::Install);
        // an unknown stage name changes nothing
        t.feed("@@light stage dance\n");
        assert_eq!(t.progress().stage, Stage::Install);
    }

    #[test]
    fn every_stage_has_a_label() {
        for s in [Stage::Starting, Stage::InstallWsl, Stage::Download, Stage::Register, Stage::Import, Stage::Prepare, Stage::Install, Stage::Verify, Stage::Config] {
            assert!(!s.label().is_empty());
        }
    }
}

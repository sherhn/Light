//! Whether the system is light or dark, as far as the system tells it: what the logo of the window's icon (the taskbar on
//! Windows, the Dock on macOS, the panel or switcher on Linux) is chosen by. It is read in the background, never on the
//! window's own thread, and every way of reading it may fail without harm: what is not known is `None`, and the logo
//! stays the light one, as it was before the system was asked at all.
//!
//! - Windows: `SystemUsesLightTheme` in the user's personalization (the colour of the taskbar).
//! - macOS: `defaults read -g AppleInterfaceStyle` (`Dark`, or no such key in the light mode).
//! - Linux: the freedesktop portal's `color-scheme`, else the `color-scheme` and the theme name of GNOME's settings. A
//!   GNOME desktop keeps a dark panel and switcher in both modes, so it is `dark` whatever the applications' scheme.
//!
//! The readers that run commands are used only on the system they are written for, the parts that understand what the
//! commands say are tested everywhere.
// each system runs its own readers; the parts that understand what the others say are compiled (and tested) everywhere
#![allow(dead_code)]

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// How often the system is asked again, and how long it is left alone after reading nothing three times in a row (the
/// way it is read is not there on this system: no more processes are started for it).
const READ_EVERY: Duration = Duration::from_secs(5);
const GIVE_UP_FOR: Duration = Duration::from_secs(600);
const FAILURES_BEFORE_GIVING_UP: u32 = 3;
/// How long a command may take before it is ended.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// Asks the system now and then, without waiting for the answer: the last answer is what [`Probe::refresh`] returns.
pub struct Probe {
    reader: fn() -> Option<bool>,
    /// `true`: the system is light. `None`: not known (yet).
    last: Option<bool>,
    pending: Option<Receiver<Option<bool>>>,
    next_read: Option<Instant>,
    failures: u32,
}

impl Probe {
    pub fn new(reader: fn() -> Option<bool>) -> Probe {
        Probe { reader, last: None, pending: None, next_read: None, failures: 0 }
    }

    /// The probe of this system.
    pub fn system() -> Probe {
        Probe::new(read_system)
    }

    /// Ask again at the next [`Probe::refresh`], whenever that was last done.
    pub fn read_now(&mut self) {
        self.next_read = None;
        self.failures = 0;
    }

    /// Take an answer that has come, start asking again when it is time, and say what is known: `Some(true)` when the
    /// system is light. `wake` is called, from the thread that asked, when an answer is ready (the window needs a frame
    /// to use it). An answer of nothing never replaces one that was known.
    pub fn refresh(&mut self, wake: impl Fn() + Send + 'static) -> Option<bool> {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(answer) => {
                    self.pending = None;
                    match answer {
                        Some(light) => {
                            self.last = Some(light);
                            self.failures = 0;
                        }
                        None => self.failures += 1,
                    }
                    let wait = if self.failures >= FAILURES_BEFORE_GIVING_UP { GIVE_UP_FOR } else { READ_EVERY };
                    self.next_read = Some(Instant::now() + wait);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.failures += 1;
                }
            }
        }
        if self.pending.is_none() && self.next_read.is_none_or(|at| Instant::now() >= at) {
            let (tx, rx) = mpsc::channel();
            let reader = self.reader;
            let started = std::thread::Builder::new().name("system-theme".into()).spawn(move || {
                // a reader that panics reads nothing
                let answer = std::panic::catch_unwind(reader).unwrap_or(None);
                let _ = tx.send(answer);
                wake();
            });
            match started {
                Ok(_) => self.pending = Some(rx),
                // no thread to ask with: not asked again for a long while
                Err(_) => self.next_read = Some(Instant::now() + GIVE_UP_FOR),
            }
        }
        self.last
    }
}

fn read_system() -> Option<bool> {
    #[cfg(windows)]
    return crate::brand::taskbar_is_light();
    #[cfg(target_os = "macos")]
    return macos_light();
    #[cfg(not(any(windows, target_os = "macos")))]
    return linux_light();
}

// ------------------------------------------------------------------------------------------------ running commands

/// What a command said.
struct Said {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Run `program` and wait for it, at most [`COMMAND_TIMEOUT`]. A program that is not there, that does not end in time or
/// that cannot be read says nothing (`None`).
fn run(program: &str, args: &[&str]) -> Option<Said> {
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < COMMAND_TIMEOUT => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    // what these commands say fits a pipe's buffer, so it can be read after they have ended
    fn read_all(pipe: Option<impl std::io::Read>) -> String {
        let mut bytes = Vec::new();
        if let Some(pipe) = pipe {
            let _ = pipe.take(64 * 1024).read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
    let stdout = read_all(child.stdout.take());
    let stderr = read_all(child.stderr.take());
    Some(Said { success: status.success(), stdout, stderr })
}

// ------------------------------------------------------------------------------------------------ what they mean

/// What the freedesktop portal or GNOME's settings call the preference for a scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scheme {
    Dark,
    Light,
    /// The user has said nothing: the next source decides.
    NoPreference,
}

/// `defaults read -g AppleInterfaceStyle`: `Dark` in the dark mode; in the light mode the key is not there, and the
/// command fails saying so. Any other failure says nothing about the mode.
fn parse_apple_interface_style(success: bool, stdout: &str, stderr: &str) -> Option<bool> {
    if success {
        return stdout.trim().eq_ignore_ascii_case("dark").then_some(false);
    }
    stderr.contains("does not exist").then_some(true)
}

/// What `gdbus call ... org.freedesktop.portal.Settings.Read org.freedesktop.appearance color-scheme` prints:
/// `(<<uint32 1>>,)`, where 1 is dark, 2 light and 0 no preference.
fn parse_portal_color_scheme(output: &str) -> Option<Scheme> {
    let after = output.rsplit_once("uint32")?.1.trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    match digits.parse::<u32>().ok()? {
        1 => Some(Scheme::Dark),
        2 => Some(Scheme::Light),
        0 => Some(Scheme::NoPreference),
        _ => None,
    }
}

/// What `gsettings get org.gnome.desktop.interface color-scheme` prints: `'prefer-dark'`, `'prefer-light'`, `'default'`.
fn parse_gsettings_color_scheme(output: &str) -> Option<Scheme> {
    match output.trim().trim_matches('\'') {
        "prefer-dark" => Some(Scheme::Dark),
        "prefer-light" => Some(Scheme::Light),
        "default" => Some(Scheme::NoPreference),
        _ => None,
    }
}

/// A theme named `Adwaita-dark` or `Yaru-dark` is a dark one (`gsettings get org.gnome.desktop.interface gtk-theme`).
fn gtk_theme_is_dark(output: &str) -> Option<bool> {
    let name = output.trim().trim_matches('\'');
    (!name.is_empty()).then(|| name.to_lowercase().contains("dark"))
}

/// Does the desktop (`XDG_CURRENT_DESKTOP`, a list such as `ubuntu:GNOME`) keep a dark panel and switcher whatever the
/// applications' colour scheme is? GNOME does, and so do the desktops built on it; GNOME Classic has a light one.
fn desktop_keeps_a_dark_panel(desktop: &str) -> bool {
    desktop.split(':').any(|part| part.eq_ignore_ascii_case("gnome") || part.eq_ignore_ascii_case("unity"))
}

// ------------------------------------------------------------------------------------------------ the readers

#[cfg(target_os = "macos")]
fn macos_light() -> Option<bool> {
    let said = run("defaults", &["read", "-g", "AppleInterfaceStyle"])?;
    parse_apple_interface_style(said.success, &said.stdout, &said.stderr)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn linux_light() -> Option<bool> {
    if desktop_keeps_a_dark_panel(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()) {
        return Some(false);
    }
    let decided = |scheme: Scheme| match scheme {
        Scheme::Dark => Some(Some(false)),
        Scheme::Light => Some(Some(true)),
        Scheme::NoPreference => None,
    };
    // the portal every current desktop answers, then GNOME's own settings, then the name of the theme
    let portal = ["call", "--session", "--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop", "--method", "org.freedesktop.portal.Settings.Read", "org.freedesktop.appearance", "color-scheme"];
    if let Some(said) = run("gdbus", &portal).filter(|s| s.success)
        && let Some(answer) = parse_portal_color_scheme(&said.stdout).and_then(decided)
    {
        return answer;
    }
    if let Some(said) = run("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"]).filter(|s| s.success)
        && let Some(answer) = parse_gsettings_color_scheme(&said.stdout).and_then(decided)
    {
        return answer;
    }
    let said = run("gsettings", &["get", "org.gnome.desktop.interface", "gtk-theme"]).filter(|s| s.success)?;
    gtk_theme_is_dark(&said.stdout).map(|dark| !dark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_mode_of_macos_is_read_from_the_defaults_command() {
        assert_eq!(parse_apple_interface_style(true, "Dark\n", ""), Some(false));
        assert_eq!(parse_apple_interface_style(true, "dark", ""), Some(false));
        // the light mode: the key is not there
        assert_eq!(parse_apple_interface_style(false, "", "\nThe domain/default pair of (kCFPreferencesAnyApplication, AppleInterfaceStyle) does not exist\n"), Some(true));
        // anything else says nothing
        assert_eq!(parse_apple_interface_style(false, "", "defaults: command crashed"), None);
        assert_eq!(parse_apple_interface_style(true, "Something else", ""), None);
        assert_eq!(parse_apple_interface_style(true, "", ""), None);
    }

    #[test]
    fn the_portal_and_gnome_settings_are_understood() {
        assert_eq!(parse_portal_color_scheme("(<<uint32 1>>,)\n"), Some(Scheme::Dark));
        assert_eq!(parse_portal_color_scheme("(<uint32 2>,)"), Some(Scheme::Light));
        assert_eq!(parse_portal_color_scheme("(<<uint32 0>>,)"), Some(Scheme::NoPreference));
        assert_eq!(parse_portal_color_scheme("(<<uint32 7>>,)"), None);
        assert_eq!(parse_portal_color_scheme("Error: no such method"), None);
        // what a system without the portal really says (WSL's Ubuntu), and what its settings say
        assert_eq!(parse_portal_color_scheme("Error: GDBus.Error:org.freedesktop.DBus.Error.ServiceUnknown: The name org.freedesktop.portal.Desktop was not provided by any .service files"), None);
        assert_eq!(parse_portal_color_scheme(""), None);
        assert_eq!(parse_gsettings_color_scheme("'prefer-dark'\n"), Some(Scheme::Dark));
        assert_eq!(parse_gsettings_color_scheme("'prefer-light'"), Some(Scheme::Light));
        assert_eq!(parse_gsettings_color_scheme("'default'"), Some(Scheme::NoPreference));
        assert_eq!(parse_gsettings_color_scheme("No such schema"), None);
        assert_eq!(gtk_theme_is_dark("'Adwaita-dark'\n"), Some(true));
        assert_eq!(gtk_theme_is_dark("'Breeze'"), Some(false));
        assert_eq!(gtk_theme_is_dark("''"), None);
    }

    #[test]
    fn gnome_and_the_desktops_built_on_it_keep_a_dark_panel() {
        for dark in ["GNOME", "ubuntu:GNOME", "pop:GNOME", "Unity", "Budgie:GNOME"] {
            assert!(desktop_keeps_a_dark_panel(dark), "{dark}");
        }
        for follows in ["KDE", "XFCE", "X-Cinnamon", "MATE", "LXQt", "GNOME-Classic", "sway", ""] {
            assert!(!desktop_keeps_a_dark_panel(follows), "{follows}");
        }
    }

    static READS: AtomicUsize = AtomicUsize::new(0);

    fn answers_light() -> Option<bool> {
        READS.fetch_add(1, Ordering::SeqCst);
        Some(true)
    }

    fn knows_nothing() -> Option<bool> {
        None
    }

    fn panics() -> Option<bool> {
        panic!("a reader that goes wrong");
    }

    /// Call `refresh` until the answer is something (or the time is up).
    fn wait_for(probe: &mut Probe) -> Option<bool> {
        let start = Instant::now();
        loop {
            let known = probe.refresh(|| {});
            if known.is_some() || start.elapsed() > Duration::from_secs(10) {
                return known;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_answer_comes_without_waiting_for_it_and_is_asked_for_again_only_in_its_time() {
        let mut probe = Probe::new(answers_light);
        // the first look starts the asking and does not know yet (or already does, when the reader was quick)
        let _ = probe.refresh(|| {});
        assert_eq!(wait_for(&mut probe), Some(true));
        let reads = READS.load(Ordering::SeqCst);
        // not asked again before its time, however often it is looked at
        for _ in 0..50 {
            assert_eq!(probe.refresh(|| {}), Some(true));
        }
        assert_eq!(READS.load(Ordering::SeqCst), reads);
        // unless it is asked to
        probe.read_now();
        let start = Instant::now();
        while READS.load(Ordering::SeqCst) == reads && start.elapsed() < Duration::from_secs(10) {
            probe.refresh(|| {});
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(READS.load(Ordering::SeqCst) > reads);
    }

    #[test]
    fn a_reader_that_knows_nothing_or_goes_wrong_changes_nothing_and_stops_being_asked() {
        for reader in [knows_nothing as fn() -> Option<bool>, panics] {
            let mut probe = Probe::new(reader);
            let start = Instant::now();
            // three readings of nothing: then it is left alone for a long while
            while probe.failures < FAILURES_BEFORE_GIVING_UP && start.elapsed() < Duration::from_secs(20) {
                probe.next_read = None;
                assert_eq!(probe.refresh(|| {}), None);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(probe.failures, FAILURES_BEFORE_GIVING_UP);
            assert!(probe.next_read.is_some_and(|at| at > Instant::now() + READ_EVERY * 10));
            assert_eq!(probe.refresh(|| {}), None);
        }
    }

    #[test]
    fn what_was_known_is_not_lost_to_a_later_nothing() {
        let mut probe = Probe::new(knows_nothing);
        probe.last = Some(false);
        let start = Instant::now();
        while probe.failures == 0 && start.elapsed() < Duration::from_secs(10) {
            assert_eq!(probe.refresh(|| {}), Some(false));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(probe.failures, 1);
        assert_eq!(probe.refresh(|| {}), Some(false));
    }

    #[test]
    fn a_missing_program_and_a_slow_one_say_nothing() {
        assert!(run("light-no-such-program-anywhere", &[]).is_none());
        // a program that does not end in time is ended
        let (program, args): (&str, &[&str]) = if cfg!(windows) { ("ping", &["-n", "30", "127.0.0.1"]) } else { ("sleep", &["30"]) };
        let started = Instant::now();
        assert!(run(program, args).is_none());
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}

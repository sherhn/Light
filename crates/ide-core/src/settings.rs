//! Persistent IDE settings (JSON in the platform config directory).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    /// JetBrains "New UI" dark.
    #[default]
    #[serde(alias = "Darcula")]
    Dark,
    Light,
    /// One of the two palettes with the colours the user changed (see [`CustomTheme`]).
    Custom,
}

impl Theme {
    pub fn toggled(self) -> Theme {
        match self {
            Theme::Dark | Theme::Custom => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }
}

/// The user's own theme: the dark or the light palette with some colours changed. A colour is named by the key of its
/// slot in the palette (`accent`, `comment`, ...) and written as `#RRGGBB`, or `#RRGGBBAA` when it is translucent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomTheme {
    /// Start from the dark palette (`true`) or from the light one.
    pub dark: bool,
    pub colors: BTreeMap<String, String>,
}

impl Default for CustomTheme {
    fn default() -> Self {
        CustomTheme { dark: true, colors: BTreeMap::new() }
    }
}

impl CustomTheme {
    /// The colour changed for `key`, if it was (and is written in a form [`parse_color`] reads).
    pub fn get(&self, key: &str) -> Option<[u8; 4]> {
        self.colors.get(key).and_then(|s| parse_color(s))
    }

    pub fn set(&mut self, key: &str, rgba: [u8; 4]) {
        self.colors.insert(key.to_string(), format_color(rgba));
    }

    /// Back to the palette's own colour.
    pub fn reset(&mut self, key: &str) {
        self.colors.remove(key);
    }

    pub fn reset_all(&mut self) {
        self.colors.clear();
    }
}

/// Read `#RRGGBB` or `#RRGGBBAA` (the `#` is optional, upper or lower case) as unmultiplied red, green, blue, alpha.
pub fn parse_color(s: &str) -> Option<[u8; 4]> {
    let h = s.trim().trim_start_matches('#');
    if !(h.len() == 6 || h.len() == 8) || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?, if h.len() == 8 { byte(6)? } else { 255 }])
}

/// `#RRGGBB` for an opaque colour, `#RRGGBBAA` for a translucent one.
pub fn format_color([r, g, b, a]: [u8; 4]) -> String {
    if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
}

/// A group of Linux packages the user added to the developer tools on the Linux page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomTool {
    pub name: String,
    /// The packages, separated by spaces, as the distribution's package manager names them.
    pub packages: String,
    /// The programs that show it is installed, separated by spaces; empty: the packages themselves are looked for.
    pub programs: String,
}

/// What of the developer tools is put into the project's Linux: everything the modules ask for unless it is left out
/// here, and the tools the user added.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LinuxTools {
    /// Packages that are not installed (a package that is not listed is).
    pub skipped: Vec<String>,
    pub custom: Vec<CustomTool>,
}

impl LinuxTools {
    pub fn is_chosen(&self, package: &str) -> bool {
        !self.skipped.iter().any(|p| p == package)
    }

    /// Choose a package for installing (`true`) or leave it out.
    pub fn choose(&mut self, package: &str, chosen: bool) {
        self.skipped.retain(|p| p != package);
        if !chosen {
            self.skipped.push(package.to_string());
        }
    }
}

/// Language of the user interface.
/// Which version of the logo is shown: the light one suits dark backgrounds, the dark one light backgrounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LogoStyle {
    /// Inside the window it follows the theme; the window's icon follows the colour of the system's taskbar.
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Language {
    #[default]
    English,
    Russian,
}

/// Where Lisp, the REPL and the terminal run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RunEnv {
    #[default]
    Windows,
    /// Inside a Linux distribution of the Windows Subsystem for Linux.
    Wsl,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSession {
    pub files: Vec<String>,
    pub active: Option<String>,
    pub expanded: Vec<String>,
    /// Where each file was left: a file opened again (or after a restart) comes back there.
    pub positions: BTreeMap<String, FilePos>,
}

/// Where a file was left in the editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilePos {
    /// Byte offset of the caret.
    pub caret: usize,
    /// The (fractional) line at the top of the view.
    pub top_line: f32,
}

/// The most files whose places a project remembers.
pub const MAX_POSITIONS: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    /// The colours of [`Theme::Custom`].
    pub custom_theme: CustomTheme,
    pub language: Language,
    pub font_size: f32,
    pub show_line_numbers: bool,
    pub show_whitespace: bool,
    /// Code overview (minimap) next to the scrollbar.
    pub minimap: bool,
    pub rainbow_brackets: bool,
    /// Show the completion popup automatically while typing.
    pub auto_popup: bool,
    pub recent_projects: Vec<String>,
    pub last_project: Option<String>,
    /// Explicit path to the Lisp binary; empty = auto-detect.
    pub lisp_path: String,
    pub run_env: RunEnv,
    /// WSL distribution used when `run_env` is `Wsl`.
    pub wsl_distro: String,
    /// SBCL inside the distribution (command or absolute path); empty = `sbcl`.
    pub wsl_sbcl: String,
    /// Where Windows drives are mounted inside the distribution; empty = `/mnt/`.
    pub wsl_mount_root: String,
    /// Which developer tools are put into Linux, and the ones the user added.
    pub linux_tools: LinuxTools,
    /// Where modules are downloaded from (a web address or a folder); empty: the published registry.
    pub registry_url: String,
    /// The first-run welcome (choosing modules) has been shown.
    pub modules_welcome_done: bool,
    /// What a Markdown or reStructuredText tab shows: `code`, `split` (code and the rendered document) or `preview`.
    pub markup_preview: String,
    /// Default parent folder for "Get from Version Control"; empty = ~/LightProjects.
    pub clone_dir: String,
    /// The GitHub login last signed in with or looked up (prefills the GitHub tab); never a secret.
    pub github_user: String,
    /// Sign in again on its own when the GitHub tab opens (only after a login the CLI or Git keeps for us).
    pub github_autologin: bool,
    pub project_sessions: BTreeMap<String, ProjectSession>,
    pub open_files: Vec<String>,
    pub active_file: Option<String>,
    pub show_project_panel: bool,
    pub show_bottom_panel: bool,
    /// The project tree shows the size and the time of the last change beside each file.
    pub show_file_details: bool,
    pub logo: LogoStyle,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::Dark,
            custom_theme: CustomTheme::default(),
            language: Language::English,
            font_size: 14.0,
            show_line_numbers: true,
            show_whitespace: false,
            minimap: true,
            rainbow_brackets: true,
            auto_popup: true,
            recent_projects: vec![],
            last_project: None,
            lisp_path: String::new(),
            run_env: RunEnv::Windows,
            wsl_distro: String::new(),
            wsl_sbcl: String::new(),
            wsl_mount_root: String::new(),
            linux_tools: LinuxTools::default(),
            registry_url: String::new(),
            modules_welcome_done: false,
            markup_preview: "split".into(),
            clone_dir: String::new(),
            github_user: String::new(),
            github_autologin: false,
            project_sessions: BTreeMap::new(),
            open_files: vec![],
            active_file: None,
            show_project_panel: true,
            show_bottom_panel: true,
            show_file_details: true,
            logo: LogoStyle::Auto,
        }
    }
}

pub const MIN_FONT: f32 = 8.0;
pub const MAX_FONT: f32 = 40.0;

impl Settings {
    pub fn config_dir() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("LIGHT_CONFIG_DIR") {
            return Some(PathBuf::from(p));
        }
        dirs::config_dir().map(|d| d.join("Light"))
    }

    pub fn load_from(path: &Path) -> Settings {
        Self::read_from(path).or_else(|| Self::read_from(&path.with_extension("json.bak")))
            .or_else(|| Self::read_from(&path.with_extension("json.tmp"))).unwrap_or_default()
    }

    fn read_from(path: &Path) -> Option<Settings> {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .map(|mut s| {
                s.font_size = s.font_size.clamp(MIN_FONT, MAX_FONT);
                s
            })
    }

    pub fn load() -> Settings {
        Self::config_dir().map(|dir| Self::load_from(&dir.join("settings.json"))).unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        if Self::read_from(path).is_some() { fs::copy(path, path.with_extension("json.bak"))?; }
        // write atomically so a crash can't leave a half-written file
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }

    pub fn save(&self) -> io::Result<()> {
        match Self::config_dir() {
            Some(d) => self.save_to(&d.join("settings.json")),
            None => Ok(()),
        }
    }

    pub fn push_recent_project(&mut self, path: &str) {
        self.recent_projects.retain(|p| p != path);
        self.recent_projects.insert(0, path.to_string());
        self.recent_projects.truncate(30);
        self.last_project = Some(path.to_string());
    }

    pub fn zoom(&mut self, delta: f32) {
        self.font_size = (self.font_size + delta).clamp(MIN_FONT, MAX_FONT);
    }

    /// What the dark/light switch goes to: the other of the two, and from the user's own theme to the opposite of the
    /// palette it is made from.
    pub fn toggled_theme(&self) -> Theme {
        match self.theme {
            Theme::Custom if self.custom_theme.dark => Theme::Light,
            Theme::Custom => Theme::Dark,
            t => t.toggled(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn a_damaged_profile_recovers_the_last_valid_backup() {
        let temp = TempDir::new("profile-backup");
        let file = temp.path().join("settings.json");
        let mut settings = Settings::default();
        settings.push_recent_project("retained-project");
        settings.save_to(&file).unwrap();
        let mut updated = settings.clone(); updated.font_size = 20.0;
        updated.save_to(&file).unwrap();
        fs::write(&file, "{incomplete").unwrap();
        assert_eq!(Settings::load_from(&file), settings);
    }

    #[test]
    fn roundtrip_and_defaults() {
        let d = TempDir::new("settings");
        let f = d.path().join("cfg/settings.json");
        let mut s = Settings { theme: Theme::Light, font_size: 17.0, ..Settings::default() };
        s.push_recent_project("E:/p");
        s.save_to(&f).unwrap();
        let back = Settings::load_from(&f);
        assert_eq!(back, s);
        assert_eq!(Settings::load_from(&d.path().join("missing.json")), Settings::default());
    }

    #[test]
    fn tolerates_garbage_and_partial_files() {
        let d = TempDir::new("settings2");
        let f = d.path().join("s.json");
        fs::write(&f, "{ not json").unwrap();
        assert_eq!(Settings::load_from(&f), Settings::default());
        fs::write(&f, r#"{"theme":"Light","font_size":999}"#).unwrap();
        let s = Settings::load_from(&f);
        assert_eq!(s.theme, Theme::Light);
        assert_eq!(s.font_size, MAX_FONT, "clamped");
        assert!(s.show_line_numbers, "missing fields use defaults");
    }

    #[test]
    fn wsl_settings_default_to_windows_and_survive_a_roundtrip() {
        let d = TempDir::new("settings3");
        let f = d.path().join("s.json");
        fs::write(&f, r#"{"theme":"Dark"}"#).unwrap();
        assert_eq!(Settings::load_from(&f).run_env, RunEnv::Windows, "older files run on Windows");
        let s = Settings { run_env: RunEnv::Wsl, wsl_distro: "Ubuntu-24.04".into(), wsl_sbcl: "/usr/bin/sbcl".into(), ..Settings::default() };
        s.save_to(&f).unwrap();
        assert_eq!(Settings::load_from(&f), s);
    }

    #[test]
    fn recent_projects_dedupe_and_cap() {
        let mut s = Settings::default();
        for i in 0..45 {
            s.push_recent_project(&format!("p{i}"));
        }
        s.push_recent_project("p5");
        assert_eq!(s.recent_projects.len(), 30);
        assert_eq!(s.recent_projects[0], "p5");
        assert_eq!(s.recent_projects.iter().filter(|p| *p == "p5").count(), 1);
    }

    #[test]
    fn colours_are_read_and_written_as_hex() {
        assert_eq!(parse_color("#2DBE60"), Some([0x2D, 0xBE, 0x60, 255]));
        assert_eq!(parse_color("  2dbe60 "), Some([0x2D, 0xBE, 0x60, 255]));
        assert_eq!(parse_color("#2DBE6034"), Some([0x2D, 0xBE, 0x60, 0x34]));
        for bad in ["", "#", "#2DBE6", "#2DBE60F", "#GGGGGG", "red", "#2DBE60 34"] {
            assert_eq!(parse_color(bad), None, "{bad:?}");
        }
        assert_eq!(format_color([0x2D, 0xBE, 0x60, 255]), "#2DBE60");
        assert_eq!(format_color([1, 2, 3, 4]), "#01020304");
        for c in [[0, 0, 0, 0], [255, 255, 255, 255], [12, 200, 7, 99]] {
            assert_eq!(parse_color(&format_color(c)), Some(c));
        }
    }

    #[test]
    fn custom_theme_survives_a_roundtrip_and_older_files_have_none() {
        let d = TempDir::new("settings4");
        let f = d.path().join("s.json");
        fs::write(&f, r#"{"theme":"Light"}"#).unwrap();
        let s = Settings::load_from(&f);
        assert!(s.custom_theme.dark && s.custom_theme.colors.is_empty(), "no custom colours in an older file");
        let mut s = Settings { theme: Theme::Custom, ..Settings::default() };
        s.custom_theme.dark = false;
        s.custom_theme.set("accent", [1, 2, 3, 255]);
        s.custom_theme.set("hover", [9, 8, 7, 20]);
        s.save_to(&f).unwrap();
        let back = Settings::load_from(&f);
        assert_eq!(back, s);
        assert_eq!(back.custom_theme.get("accent"), Some([1, 2, 3, 255]));
        assert_eq!(back.custom_theme.get("hover"), Some([9, 8, 7, 20]));
        let mut c = back.custom_theme;
        c.reset("accent");
        assert_eq!(c.get("accent"), None);
        c.reset_all();
        assert!(c.colors.is_empty());
    }

    #[test]
    fn a_broken_colour_in_the_file_is_ignored() {
        let d = TempDir::new("settings5");
        let f = d.path().join("s.json");
        fs::write(&f, r##"{"theme":"Custom","custom_theme":{"dark":false,"colors":{"accent":"nonsense","text":"#102030"}}}"##).unwrap();
        let s = Settings::load_from(&f);
        assert_eq!(s.theme, Theme::Custom);
        assert_eq!(s.custom_theme.get("accent"), None);
        assert_eq!(s.custom_theme.get("text"), Some([0x10, 0x20, 0x30, 255]));
    }

    #[test]
    fn the_theme_switch_leaves_the_custom_theme_for_the_opposite_palette() {
        let mut s = Settings::default();
        assert_eq!(s.toggled_theme(), Theme::Light);
        s.theme = Theme::Light;
        assert_eq!(s.toggled_theme(), Theme::Dark);
        s.theme = Theme::Custom;
        assert_eq!(s.toggled_theme(), Theme::Light, "a dark custom theme");
        s.custom_theme.dark = false;
        assert_eq!(s.toggled_theme(), Theme::Dark, "a light custom theme");
    }

    #[test]
    fn linux_tools_are_all_chosen_until_one_is_left_out() {
        let mut t = LinuxTools::default();
        assert!(t.is_chosen("gcc") && t.is_chosen("anything"));
        t.choose("vim", false);
        t.choose("vim", false);
        t.choose("jq", false);
        assert_eq!(t.skipped, ["vim", "jq"], "left out once");
        assert!(!t.is_chosen("vim") && t.is_chosen("gcc"));
        t.choose("vim", true);
        assert!(t.is_chosen("vim") && t.skipped == ["jq"]);
    }

    #[test]
    fn linux_tools_survive_a_roundtrip_and_older_files_have_none() {
        let d = TempDir::new("settings6");
        let f = d.path().join("s.json");
        fs::write(&f, r#"{"theme":"Dark"}"#).unwrap();
        assert_eq!(Settings::load_from(&f).linux_tools, LinuxTools::default());
        let mut s = Settings::default();
        s.linux_tools.choose("doxygen", false);
        s.linux_tools.custom.push(CustomTool { name: "Libraries".into(), packages: "libssl-dev zlib1g-dev".into(), programs: String::new() });
        s.save_to(&f).unwrap();
        assert_eq!(Settings::load_from(&f), s);
        // a tool written by hand with only some of its fields
        fs::write(&f, r#"{"linux_tools":{"custom":[{"name":"Net","packages":"nmap"}]}}"#).unwrap();
        let t = Settings::load_from(&f).linux_tools;
        assert_eq!(t.custom, [CustomTool { name: "Net".into(), packages: "nmap".into(), programs: String::new() }]);
        assert!(t.skipped.is_empty());
    }

    #[test]
    fn zoom_clamps() {
        let mut s = Settings::default();
        s.zoom(1000.0);
        assert_eq!(s.font_size, MAX_FONT);
        s.zoom(-1000.0);
        assert_eq!(s.font_size, MIN_FONT);
    }
}

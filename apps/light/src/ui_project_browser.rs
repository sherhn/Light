//! Project folders in WSL, without asking users to type a Windows UNC path.
//! Directory I/O stays on a cancellable worker; replacing a request drops its old results.

use crate::scrollbars::FadingScroll as _;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Key, RichText, Vec2};

use crate::{
    app::App,
    fonts,
    i18n::tr,
    icons::Icon,
    state::Popup,
    widgets::{self, TextField},
};

const MAX_RESULTS: usize = 500;
const MAX_VISITED: usize = 20_000;
const MAX_DEPTH: usize = 8;

struct Listing {
    root: PathBuf,
    folders: Vec<PathBuf>,
    limited: bool,
    skipped: usize,
}

struct Request {
    rx: mpsc::Receiver<Result<Listing, String>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Request {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct ProjectBrowser {
    distro: String,
    path: String,
    query: String,
    hidden: bool,
    recursive: bool,
    current: Option<PathBuf>,
    selected: Option<PathBuf>,
    folders: Vec<PathBuf>,
    request: Option<Request>,
    changed_at: Option<Instant>,
    error: Option<String>,
    limited: bool,
    skipped: usize,
    just_opened: bool,
}

impl ProjectBrowser {
    pub fn idle(&self) -> bool {
        self.request.is_none() && self.changed_at.is_none()
    }

    fn new(distro: String) -> Self {
        Self {
            distro,
            path: "~".into(),
            query: String::new(),
            hidden: false,
            recursive: true,
            current: None,
            selected: None,
            folders: vec![],
            request: None,
            changed_at: None,
            error: None,
            limited: false,
            skipped: 0,
            just_opened: true,
        }
    }

    fn navigate(&mut self, path: String) {
        self.path = path;
        self.query.clear();
        self.current = None;
        self.start();
    }

    fn start(&mut self) {
        self.request = None; // cancels the previous scan
        self.changed_at = None;
        self.selected = None;
        self.folders.clear();
        self.error = None;
        self.limited = false;
        self.skipped = 0;
        if cfg!(windows) && self.distro.is_empty() {
            return;
        }
        let (distro, path, query) = (self.distro.clone(), self.path.clone(), self.query.clone());
        let (hidden, recursive) = (self.hidden, self.recursive);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = resolve_folder(&distro, &path)
                .and_then(|root| scan_folders(root, &query, hidden, recursive, &worker_cancel));
            let _ = tx.send(result);
        });
        self.request = Some(Request { rx, cancel });
    }

    fn poll(&mut self) {
        let Some(request) = &self.request else { return };
        let result = match request.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(tr("Folder reader stopped. Try refreshing.").into())
            }
        };
        self.request = None;
        match result {
            Ok(listing) => {
                self.path = display_path(&listing.root);
                self.current = Some(listing.root);
                self.folders = listing.folders;
                self.limited = listing.limited;
                self.skipped = listing.skipped;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

fn display_path(path: &Path) -> String {
    ide_core::wsl::wsl_path(path)
        .map(|p| p.linux)
        .unwrap_or_else(|| path.display().to_string())
}

fn resolve_folder(distro: &str, input: &str) -> Result<PathBuf, String> {
    let path = if input == "~" || input.starts_with("~/") {
        let home = if cfg!(windows) {
            lisp_runtime::wsl::home_dir(distro)?
        } else {
            crate::input::dirs_home().to_string_lossy().into_owned()
        };
        format!("{}{}", home.trim_end_matches('/'), &input[1..])
    } else {
        input.to_string()
    };
    if !path.starts_with('/') {
        return Err(tr("Enter an absolute Linux path or a path starting with ~/.").into());
    }
    // Keep Up navigation predictable even after entering /home/me/../other by hand.
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let path = format!("/{}", parts.join("/"));
    Ok(if cfg!(windows) {
        ide_core::wsl::windows_path(distro, &path)
    } else {
        PathBuf::from(path)
    })
}

/// Searches directory names, never file contents or symlink targets. An empty query lists one level.
fn scan_folders(
    root: PathBuf,
    query: &str,
    hidden: bool,
    recursive: bool,
    cancel: &AtomicBool,
) -> Result<Listing, String> {
    let mut listing = Listing {
        root: root.clone(),
        folders: vec![],
        limited: false,
        skipped: 0,
    };
    let query = query.to_lowercase();
    let searching = !query.is_empty() && recursive;
    let started = Instant::now();
    let mut pending = std::collections::VecDeque::from([(root.clone(), 0)]);
    let mut visited = 0;
    while let Some((dir, depth)) = pending.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Err(String::new());
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if dir == root => return Err(format!("{}: {error}", display_path(&dir))),
            Err(_) => {
                listing.skipped += 1;
                continue;
            }
        };
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(String::new());
            }
            visited += 1;
            if visited > MAX_VISITED || started.elapsed() > Duration::from_secs(10) {
                listing.limited = true;
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    listing.skipped += 1;
                    continue;
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => {
                    listing.skipped += 1;
                    continue;
                }
            };
            // Symlink folders remain selectable; do not recurse through them (cycles, mounts).
            if !kind.is_dir() && !(kind.is_symlink() && entry.path().is_dir()) {
                continue;
            }
            let path = entry.path();
            if query.is_empty() || name.to_lowercase().contains(&query) {
                listing.folders.push(path.clone());
                if listing.folders.len() >= MAX_RESULTS {
                    listing.limited = true;
                    break;
                }
            }
            if searching
                && kind.is_dir()
                && !matches!(
                    name.as_str(),
                    ".git" | "node_modules" | "target" | ".venv" | "venv" | "__pycache__"
                )
            {
                if depth < MAX_DEPTH - 1 {
                    pending.push_back((path, depth + 1));
                } else {
                    listing.limited = true;
                }
            }
        }
        if listing.folders.len() >= MAX_RESULTS
            || visited > MAX_VISITED
            || started.elapsed() > Duration::from_secs(10)
        {
            break;
        }
    }
    listing.folders.sort_by_cached_key(|p| {
        p.strip_prefix(&root)
            .unwrap_or(p)
            .to_string_lossy()
            .to_lowercase()
    });
    Ok(listing)
}

impl App {
    pub fn open_project_browser(&mut self) {
        if cfg!(windows) && self.wsl.probe.is_none() && !self.wsl.probing() {
            self.wsl_probe();
        }
        let mut browser = ProjectBrowser::new(self.suggested_distro().unwrap_or_default());
        if let Some(root) = self.project_root() {
            if let Some(path) = ide_core::wsl::wsl_path(root) {
                browser.distro = path.distro;
                browser.path = path.linux;
            } else if !cfg!(windows) {
                browser.path = root.display().to_string();
            }
        }
        browser.start();
        self.popup = Popup::ProjectBrowser(Box::new(browser));
    }

    pub fn draw_project_browser(&mut self, ctx: &egui::Context) {
        if !matches!(self.popup, Popup::ProjectBrowser(_)) {
            return;
        }
        let Popup::ProjectBrowser(mut st) = std::mem::replace(&mut self.popup, Popup::None) else {
            return;
        };
        st.poll();
        let distros = self.wsl.distros();
        if cfg!(windows) && st.distro.is_empty() && !distros.is_empty() {
            st.distro = self
                .suggested_distro()
                .unwrap_or_else(|| distros[0].clone());
            st.start();
        }
        if st
            .changed_at
            .is_some_and(|time| time.elapsed() >= Duration::from_millis(350))
        {
            st.start();
        }
        let pal = self.pal;
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
        let mut native = false;
        let mut setup = false;
        let mut open = None;
        let mut navigate = None;
        let mut selection_moved = false;
        // Keep arrows in the address field for editing; elsewhere they select folders.
        let address_id = egui::Id::unique("project_browser_address");
        if st.request.is_none()
            && st.changed_at.is_none()
            && !ctx.memory(|m| m.has_focus(address_id))
        {
            let down = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
            let up = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
            if (down || up) && !st.folders.is_empty() {
                let current = st
                    .selected
                    .as_ref()
                    .and_then(|selected| st.folders.iter().position(|folder| folder == selected));
                let next = match current {
                    Some(index) if down => (index + 1).min(st.folders.len() - 1),
                    Some(index) => index.saturating_sub(1),
                    None if down => 0,
                    None => st.folders.len() - 1,
                };
                st.selected = Some(st.folders[next].clone());
                selection_moved = true;
            }
            if st.selected.is_some()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter))
            {
                navigate = st.selected.as_ref().map(|p| display_path(p));
            }
            if st.error.is_none()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, Key::Enter))
            {
                open = st.selected.as_ref().or(st.current.as_ref()).cloned();
            }
        }
        egui::Area::new(egui::Id::unique("project_browser")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            widgets::glass_frame(&pal).show(ui, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.0).clamp(280.0, 760.0));
                egui::ScrollArea::vertical().id_salt("project_browser_dialog")
                    .max_height((ctx.content_rect().height() - 180.0).max(140.0))
                    .min_scrolled_height((ctx.content_rect().height() - 200.0).clamp(120.0, 460.0))
                    .show_fading(ui, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.0).clamp(280.0, 760.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Open Project")).font(fonts::semibold(17.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close |= widgets::icon_button(ui, &pal, Icon::Close, 28.0, pal.text_dim, tr("Close (Esc)")).clicked();
                        if cfg!(windows) { native |= widgets::secondary_button(ui, &pal, tr("Windows folder...")).clicked(); }
                    });
                });
                ui.add_space(10.0);
                if cfg!(windows) {
                    ui.horizontal(|ui| {
                        ui.label(tr("Linux (WSL)"));
                        egui::ComboBox::from_id_salt("project_distro").selected_text(&st.distro).show_ui(ui, |ui| {
                            for distro in &distros {
                                if ui.selectable_value(&mut st.distro, distro.clone(), distro).changed() { navigate = Some("~".into()); }
                            }
                        });
                        setup |= ui.small_button(tr("Linux (WSL) setup...")).clicked();
                    });
                }
                if cfg!(windows) && st.distro.is_empty() {
                    ui.add_space(16.0);
                    if self.wsl.probing() { ui.spinner(); }
                    else { ui.label(tr("No Linux distribution is installed yet")); }
                } else {
                    ui.horizontal(|ui| {
                        if widgets::secondary_button(ui, &pal, tr("Home folder")).clicked() { navigate = Some("~".into()); }
                        if widgets::secondary_button(ui, &pal, "/").clicked() { navigate = Some("/".into()); }
                        if ui.add_enabled(st.current.as_ref().is_some_and(|p| display_path(p) != "/"), egui::Button::new(tr("Up one folder"))).clicked()
                            && let Some(parent) = st.current.as_ref().and_then(|p| p.parent()) { navigate = Some(display_path(parent)); }
                        if widgets::icon_button(ui, &pal, Icon::Refresh, 28.0, pal.text_dim, tr("Refresh")).clicked() { st.start(); }
                    });
                    ui.horizontal(|ui| {
                        let response = TextField::new(&mut st.path).id(address_id).hint("~/projects").icon(Icon::Folder).width((ui.available_width() - 85.0).max(100.0)).show(ui, &pal);
                        if ui.button(tr("Go")).clicked() || (response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) { navigate = Some(st.path.clone()); }
                    });
                    ui.add_space(6.0);
                    let response = TextField::new(&mut st.query).id(egui::Id::unique("project_browser_search")).hint(tr("Search folders by name...")).icon(Icon::Search).show(ui, &pal);
                    widgets::focus_once(ui, &response, &mut st.just_opened);
                    let mut changed = response.changed();
                    ui.horizontal(|ui| {
                        changed |= ui.checkbox(&mut st.recursive, tr("Search subfolders")).changed();
                        changed |= ui.checkbox(&mut st.hidden, tr("Show hidden folders")).changed();
                    });
                    if changed {
                        st.request = None;
                        st.changed_at = Some(Instant::now());
                        st.selected = None;
                    }
                    let busy = st.request.is_some() || st.changed_at.is_some();
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().id_salt("linux_project_folders").max_height((ctx.content_rect().height() - 500.0).clamp(60.0, 220.0)).min_scrolled_height(60.0).auto_shrink([false, false]).show_fading(ui, |ui| {
                        if busy { ui.horizontal(|ui| { ui.spinner(); ui.label(tr("Looking for folders...")); }); }
                        else if let Some(error) = &st.error { ui.colored_label(pal.error, error); }
                        else if st.folders.is_empty() { ui.label(tr("No folders found")); }
                        if !busy {
                            for folder in &st.folders {
                                let name = if st.query.is_empty() { folder.file_name().unwrap_or_default().to_string_lossy().into_owned() }
                                    else { folder.strip_prefix(st.current.as_deref().unwrap_or(Path::new("/"))).unwrap_or(folder).display().to_string().replace('\\', "/") };
                                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), egui::Sense::click());
                                if st.selected.as_ref() == Some(folder) || response.hovered() {
                                    ui.painter().rect_filled(rect, 6.0, if st.selected.as_ref() == Some(folder) { pal.selected } else { pal.hover });
                                }
                                crate::icons::paint(ui.painter(), egui::Rect::from_center_size(rect.left_center() + Vec2::new(16.0, 0.0), Vec2::splat(16.0)), Icon::Folder, pal.text_dim);
                                ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())).text(rect.left_center() + Vec2::new(34.0, 0.0), Align2::LEFT_CENTER, &name, fonts::ui(13.0), pal.text);
                                if response.clicked() { st.selected = Some(folder.clone()); }
                                if response.double_clicked() { navigate = Some(display_path(folder)); }
                                if st.selected.as_ref() == Some(folder) && selection_moved { response.scroll_to_me(Some(egui::Align::Center)); }
                                response.on_hover_text(display_path(folder));
                            }
                        }
                    });
                    if st.limited { ui.label(RichText::new(tr("Search limited. Open a closer folder or refine the name.")).color(pal.text_dim).size(12.0)); }
                    if st.skipped > 0 { ui.label(RichText::new(tr("Some folders could not be read.")).color(pal.text_dim).size(12.0)); }
                    if !st.query.is_empty() && st.recursive { ui.label(RichText::new(tr("Searches up to 8 levels; skips dependency and build folders.")).color(pal.text_dim).size(11.5)); }
                    let recent: Vec<_> = self.settings.recent_projects.iter().filter_map(|path| {
                        let wsl = ide_core::wsl::wsl_path(Path::new(path))?;
                        (wsl.distro == st.distro).then_some(wsl.linux)
                    }).take(4).collect();
                    if !recent.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(tr("Recent Projects"));
                            for path in recent { if ui.small_button(&path).clicked() { navigate = Some(path); } }
                        });
                    }
                }
                });
                // The action stays outside the scrolling content, including in small windows.
                ui.separator();
                let target = st.selected.as_ref().or(st.current.as_ref());
                if let Some(target) = target {
                    ui.add(egui::Label::new(RichText::new(display_path(target)).color(pal.text_dim).size(12.0)).truncate());
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled_ui(st.idle() && st.error.is_none() && target.is_some(), |ui| widgets::primary_button(ui, &pal, tr("Open Project"))).inner.on_hover_text("Ctrl+Enter").clicked() { open = target.cloned(); }
                    if st.selected.is_some() && widgets::secondary_button(ui, &pal, tr("Browse folder")).on_hover_text("Enter").clicked() { navigate = st.selected.as_ref().map(|p| display_path(p)); }
                    close |= widgets::secondary_button(ui, &pal, tr("Cancel")).clicked();
                });
            });
        });
        if close {
            return;
        }
        if native {
            if let Some(folder) = rfd::FileDialog::new()
                .set_title(tr("Open Project"))
                .pick_folder()
            {
                self.request_open_project(&folder);
                return;
            }
        } else if setup {
            self.open_settings_page(crate::ui_settings::WSL_PAGE);
            return;
        } else if let Some(path) = open {
            self.request_open_project(&path);
            return;
        } else if let Some(path) = navigate {
            st.navigate(path);
        }
        if st.request.is_some() || st.changed_at.is_some() || self.wsl.probing() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.popup = Popup::ProjectBrowser(st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "light-browser-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn folder(&self, name: &str) {
            std::fs::create_dir_all(self.0.join(name)).unwrap();
        }
        fn scan(&self, query: &str, hidden: bool, recursive: bool) -> Listing {
            scan_folders(
                self.0.clone(),
                query,
                hidden,
                recursive,
                &AtomicBool::new(false),
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let path = self.0.canonicalize().unwrap();
            assert_eq!(
                path.parent(),
                Some(std::env::temp_dir().canonicalize().unwrap().as_path())
            );
            assert!(
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("light-browser-test-")
            );
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn listing_only_contains_direct_visible_folders_in_name_order() {
        let f = Fixture::new();
        for name in ["Zoo/deep", "alpha", ".hidden"] {
            f.folder(name);
        }
        std::fs::write(f.0.join("file.txt"), "file").unwrap();
        let found = f.scan("", false, true);
        assert_eq!(found.folders, [f.0.join("alpha"), f.0.join("Zoo")]);
        assert_eq!(f.scan("", true, true).folders.len(), 3);
    }

    #[test]
    fn recursive_search_matches_names_and_skips_dependencies() {
        let f = Fixture::new();
        for name in [
            "work/My Project",
            "node_modules/My Project",
            "target/My Project",
            ".hidden/My Project",
        ] {
            f.folder(name);
        }
        let found = f.scan("my project", false, true);
        assert_eq!(found.folders, [f.0.join("work/My Project")]);
        assert!(f.scan("my project", false, false).folders.is_empty());
        assert_eq!(f.scan("MY PROJECT", true, true).folders.len(), 2);
        // Dependency folders can still be opened directly.
        assert_eq!(
            f.scan("node_modules", false, true).folders,
            [f.0.join("node_modules")]
        );
    }

    #[test]
    fn limits_cancellation_and_missing_directory_are_reported() {
        let f = Fixture::new();
        for index in 0..MAX_RESULTS + 2 {
            f.folder(&format!("project-{index}"));
        }
        let found = f.scan("", false, false);
        assert_eq!(found.folders.len(), MAX_RESULTS);
        assert!(found.limited);
        assert!(scan_folders(f.0.clone(), "", false, false, &AtomicBool::new(true)).is_err());
        assert!(
            scan_folders(
                f.0.join("missing"),
                "",
                false,
                false,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn linux_paths_are_normalized_without_interpreting_shell_characters() {
        let resolved = resolve_folder("Ubuntu", "/home/me/../some project/./$(echo hi)").unwrap();
        assert_eq!(display_path(&resolved), "/home/some project/$(echo hi)");
        assert_eq!(
            display_path(&resolve_folder("Ubuntu", "/../../").unwrap()),
            "/"
        );
        assert!(resolve_folder("Ubuntu", "relative/project").is_err());
    }

    #[test]
    fn browser_search_and_open_button_are_visible_after_layout_settles() {
        for (width, height, query) in [
            (1280.0, 800.0, ""),
            (1280.0, 800.0, "project"),
            (640.0, 400.0, "project"),
        ] {
            let ctx = egui::Context::default();
            fonts::install(&ctx);
            let mut app = App::new(ide_core::settings::Settings::default());
            app.settings.recent_projects = (0..4)
                .map(|n| {
                    ide_core::wsl::windows_path(
                        "Ubuntu",
                        &format!("/home/me/workspaces/recent-project-{n}"),
                    )
                    .to_string_lossy()
                    .into_owned()
                })
                .collect();
            let mut browser = ProjectBrowser::new("Ubuntu".into());
            browser.query = query.into();
            browser.path = "/home/me".into();
            browser.current = Some(resolve_folder("Ubuntu", "/home/me").unwrap());
            browser
                .folders
                .push(resolve_folder("Ubuntu", "/home/me/project").unwrap());
            app.popup = Popup::ProjectBrowser(Box::new(browser));
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
            for frame in 0..4 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        egui::Panel::top("browser_test_toolbar")
                            .exact_size(86.0)
                            .show(ui, |_| {});
                        egui::Panel::bottom("browser_test_status")
                            .exact_size(30.0)
                            .show(ui, |_| {});
                        app.draw_project_browser(&ctx);
                    },
                );
                if frame == 3 {
                    let wanted = if height < 500.0 {
                        vec!["Cancel"]
                    } else if query.is_empty() {
                        vec!["Search folders by name...", "project", "Cancel"]
                    } else {
                        vec!["project", "Cancel"]
                    };
                    for wanted in wanted {
                        let text = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if text.galley.job.text == wanted => {
                                    Some((
                                        shape.clip_rect,
                                        egui::Rect::from_min_size(text.pos, text.galley.size()),
                                    ))
                                }
                                _ => None,
                            })
                            .unwrap_or_else(|| panic!("missing browser control: {wanted}"));
                        assert!(
                            text.0.contains_rect(text.1),
                            "browser control clipped: {wanted}: {text:?}"
                        );
                        assert!(
                            screen.contains_rect(text.1),
                            "browser control outside viewport: {wanted}"
                        );
                    }
                }
                output.textures_delta.clear();
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_folders_are_selectable_without_following_cycles() {
        let f = Fixture::new();
        f.folder("work/project");
        std::os::unix::fs::symlink(&f.0, f.0.join("work/loop")).unwrap();
        let found = f.scan("loop", false, true);
        assert_eq!(found.folders, [f.0.join("work/loop")]);
        assert!(!found.limited);
    }
}

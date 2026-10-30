//! The Open Project dialog and the folder picker of Linux destinations: a visual explorer of the folders of this
//! computer, or of a Linux distribution in WSL (nobody types a UNC path). This computer comes first; Linux is the other
//! tab of the same dialog. Directory I/O stays on a cancellable worker; replacing a request drops its old results.

use crate::scrollbars::FadingScroll as _;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Align2, CornerRadius, Key, Pos2, Rect, RichText, Sense, Vec2};

use crate::{
    app::App,
    fonts,
    i18n::tr,
    icons::{self, Icon},
    state::Popup,
    theme::Palette,
    widgets::{self, TextField},
};

const MAX_RESULTS: usize = 500;
const MAX_VISITED: usize = 20_000;
const MAX_DEPTH: usize = 8;

/// A folder or file of a listing.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// A folder under version control (it holds `.git`): most likely a project.
    pub project: bool,
}

struct Listing {
    root: PathBuf,
    items: Vec<Item>,
    /// The items are the folders a search found below `root`, not its own contents.
    found: bool,
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

/// Which file system an explorer shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// This computer's own folders (Windows, or the system Light runs on).
    Local,
    /// A Linux distribution in WSL.
    Linux,
}

/// What the explorer is for: opening a project, or choosing a folder for something else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Open,
    Pick,
}

/// What a frame of the explorer asks for.
pub enum Outcome {
    None,
    Close,
    /// Open (or pick) this folder.
    Choose(PathBuf),
    /// Use the system's own folder dialog instead.
    Native,
    /// Open the settings of Linux (WSL).
    Setup,
}

pub struct Explorer {
    pub side: Side,
    pub purpose: Purpose,
    pub distro: String,
    /// What the address says: a Windows path, or a Linux one in WSL.
    path: String,
    editing: bool,
    query: String,
    hidden: bool,
    recursive: bool,
    current: Option<PathBuf>,
    selected: Option<PathBuf>,
    items: Vec<Item>,
    found: bool,
    /// The folders shown before, for Back.
    back: Vec<String>,
    request: Option<Request>,
    changed_at: Option<Instant>,
    error: Option<String>,
    limited: bool,
    skipped: usize,
    just_opened: bool,
}

impl Explorer {
    pub fn idle(&self) -> bool {
        self.request.is_none() && self.changed_at.is_none()
    }

    pub fn new(side: Side, purpose: Purpose, distro: String, path: String) -> Self {
        Self {
            side,
            purpose,
            distro,
            path,
            editing: false,
            query: String::new(),
            hidden: false,
            recursive: false,
            current: None,
            selected: None,
            items: vec![],
            found: false,
            back: vec![],
            request: None,
            changed_at: None,
            error: None,
            limited: false,
            skipped: 0,
            just_opened: true,
        }
    }

    /// The folder listed now, as its address says it.
    pub fn current_path(&self) -> Option<&Path> {
        self.current.as_deref()
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The folder Open (or Choose) takes: the one selected, else the one listed.
    pub fn target(&self) -> Option<&PathBuf> {
        self.selected.as_ref().or(self.current.as_ref())
    }

    pub fn select(&mut self, path: Option<PathBuf>) {
        self.selected = path;
    }

    pub fn navigate(&mut self, path: String) {
        if let Some(current) = &self.current {
            let shown = display_path(current);
            if shown != path && self.back.last() != Some(&shown) {
                self.back.push(shown);
            }
        }
        self.path = path;
        self.query.clear();
        self.editing = false;
        self.start();
    }

    fn go_back(&mut self) {
        if let Some(path) = self.back.pop() {
            self.path = path;
            self.query.clear();
            self.editing = false;
            self.start();
        }
    }

    pub fn start(&mut self) {
        self.request = None; // cancels the previous scan
        self.changed_at = None;
        self.selected = None;
        self.items.clear();
        self.found = false;
        self.error = None;
        self.limited = false;
        self.skipped = 0;
        if self.side == Side::Linux && cfg!(windows) && self.distro.is_empty() {
            return;
        }
        let (side, distro, path, query) = (self.side, self.distro.clone(), self.path.clone(), self.query.clone());
        let (hidden, recursive) = (self.hidden, self.recursive);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let root = match side {
                Side::Linux => resolve_folder(&distro, &path),
                Side::Local => resolve_local(&path),
            };
            let result = root.and_then(|root| {
                if recursive && !query.is_empty() {
                    scan_folders(root, &query, hidden, true, &worker_cancel)
                } else {
                    list_folder(root, hidden, &worker_cancel)
                }
            });
            let _ = tx.send(result);
        });
        self.request = Some(Request { rx, cancel });
    }

    pub fn poll(&mut self) {
        let Some(request) = &self.request else { return };
        let result = match request.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(tr("Folder reader stopped. Try refreshing.").into()),
        };
        self.request = None;
        match result {
            Ok(listing) => {
                self.path = display_path(&listing.root);
                self.current = Some(listing.root);
                self.items = listing.items;
                self.found = listing.found;
                self.limited = listing.limited;
                self.skipped = listing.skipped;
            }
            // a folder to start from that is not there (yet): start from home instead
            Err(_) if self.current.is_none() && self.purpose == Purpose::Pick && self.path != "~" => {
                self.path = "~".into();
                self.start();
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// The rows shown: the listing, narrowed to the names that hold the text searched for (a search of subfolders
    /// has done that already).
    fn visible(&self) -> Vec<&Item> {
        let query = self.query.trim().to_lowercase();
        self.items.iter().filter(|item| self.found || query.is_empty() || item.name.to_lowercase().contains(&query)).collect()
    }
}

/// How a folder is shown: its Linux path inside WSL, else as Windows writes it.
pub fn display_path(path: &Path) -> String {
    ide_core::wsl::wsl_path(path).map(|p| p.linux).unwrap_or_else(|| path.display().to_string())
}

/// A folder of this computer from what the address says: an absolute path, or `~` for the home folder.
fn resolve_local(input: &str) -> Result<PathBuf, String> {
    let input = input.trim().trim_matches('"');
    let path = if input == "~" {
        crate::input::dirs_home()
    } else if let Some(rest) = input.strip_prefix("~/").or_else(|| input.strip_prefix("~\\")) {
        crate::input::dirs_home().join(rest)
    } else {
        PathBuf::from(input)
    };
    // a drive letter alone (`E:`) is the drive's root
    let path = if cfg!(windows) && path.as_os_str().len() == 2 && input.ends_with(':') { PathBuf::from(format!("{input}\\")) } else { path };
    if !path.is_absolute() {
        return Err(tr("Enter a full path, such as C:\\Projects, or ~ for your home folder.").into());
    }
    Ok(path)
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

/// Is an entry hidden: a dot name, or (on Windows) marked hidden or a part of the system (`$Recycle.Bin`)?
fn is_hidden(name: &str, meta: Option<&std::fs::Metadata>) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        if meta.is_some_and(|m| m.file_attributes() & (HIDDEN | SYSTEM) != 0) {
            return true;
        }
    }
    let _ = meta;
    name.starts_with('.')
}

/// The contents of one folder: its folders first, then its files, each by name; hidden ones only on request.
fn list_folder(root: PathBuf, hidden: bool, cancel: &AtomicBool) -> Result<Listing, String> {
    let entries = std::fs::read_dir(&root).map_err(|error| format!("{}: {error}", display_path(&root)))?;
    let mut listing = Listing { root: root.clone(), items: vec![], found: false, limited: false, skipped: 0 };
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(String::new());
        }
        let Ok(entry) = entry else {
            listing.skipped += 1;
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = entry.metadata().ok();
        if !hidden && is_hidden(&name, meta.as_ref()) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            listing.skipped += 1;
            continue;
        };
        let path = entry.path();
        // Symlink folders remain selectable; their contents are listed when they are opened.
        let is_dir = kind.is_dir() || (kind.is_symlink() && path.is_dir());
        let project = is_dir && path.join(".git").exists();
        listing.items.push(Item {
            size: if is_dir { 0 } else { meta.as_ref().map_or(0, |m| m.len()) },
            modified: meta.and_then(|m| m.modified().ok()),
            path,
            name,
            is_dir,
            project,
        });
        if listing.items.len() >= MAX_RESULTS {
            listing.limited = true;
            break;
        }
    }
    listing.items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(listing)
}

/// Searches directory names, never file contents or symlink targets. An empty query lists one level.
fn scan_folders(root: PathBuf, query: &str, hidden: bool, recursive: bool, cancel: &AtomicBool) -> Result<Listing, String> {
    let mut listing = Listing { root: root.clone(), items: vec![], found: true, limited: false, skipped: 0 };
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
            if !hidden && is_hidden(&name, entry.metadata().ok().as_ref()) {
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
                let shown = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                let project = path.join(".git").exists();
                listing.items.push(Item { path: path.clone(), name: shown, is_dir: true, size: 0, modified: None, project });
                if listing.items.len() >= MAX_RESULTS {
                    listing.limited = true;
                    break;
                }
            }
            if searching && kind.is_dir() && !matches!(name.as_str(), ".git" | "node_modules" | "target" | ".venv" | "venv" | "__pycache__") {
                if depth < MAX_DEPTH - 1 {
                    pending.push_back((path, depth + 1));
                } else {
                    listing.limited = true;
                }
            }
        }
        if listing.items.len() >= MAX_RESULTS || visited > MAX_VISITED || started.elapsed() > Duration::from_secs(10) {
            break;
        }
    }
    listing.items.sort_by_cached_key(|item| item.name.to_lowercase());
    Ok(listing)
}

/// The steps of the address, each with the path it leads to: `E:` `Projects` `Cds`, or `/` `home` `me`.
fn crumbs(side: Side, shown: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if side == Side::Linux || shown.starts_with('/') {
        out.push(("/".to_string(), "/".to_string()));
        let mut acc = String::new();
        for part in shown.split('/').filter(|p| !p.is_empty()) {
            acc.push('/');
            acc.push_str(part);
            out.push((part.to_string(), acc.clone()));
        }
    } else {
        let mut acc = String::new();
        for part in shown.split(['\\', '/']).filter(|p| !p.is_empty()) {
            if acc.is_empty() {
                acc = format!("{part}\\");
            } else {
                if !acc.ends_with('\\') {
                    acc.push('\\');
                }
                acc.push_str(part);
            }
            out.push((part.to_string(), acc.clone()));
        }
    }
    out
}

/// The drives of this computer (`C:\`, `D:\`...), from the system's list of drive letters (no drive is touched).
fn drives() -> Vec<String> {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLogicalDrives() -> u32;
        }
        // SAFETY: no arguments; the result is a bit mask of drive letters
        let mask = unsafe { GetLogicalDrives() };
        (0..26).filter(|i| mask & (1 << i) != 0).map(|i| format!("{}:\\", (b'A' + i as u8) as char)).collect()
    }
    #[cfg(not(windows))]
    {
        vec!["/".to_string()]
    }
}

/// The places listed beside the folders: (icon, name, address).
fn places(side: Side, recent: &[String]) -> Vec<(Icon, String, String)> {
    let mut v = Vec::new();
    match side {
        Side::Local => {
            v.push((Icon::Home, tr("Home").to_string(), crate::input::dirs_home().display().to_string()));
            for (dir, name) in [(dirs::desktop_dir(), tr("Desktop")), (dirs::document_dir(), tr("Documents")), (dirs::download_dir(), tr("Downloads"))] {
                if let Some(dir) = dir.filter(|d| d.is_dir()) {
                    v.push((Icon::Folder, name.to_string(), dir.display().to_string()));
                }
            }
            for drive in drives() {
                v.push((Icon::Drive, drive.trim_end_matches('\\').to_string(), drive));
            }
        }
        Side::Linux => {
            v.push((Icon::Home, tr("Home").to_string(), "~".to_string()));
            v.push((Icon::Drive, "/".to_string(), "/".to_string()));
            v.push((Icon::Monitor, tr("Windows drives").to_string(), "/mnt".to_string()));
        }
    }
    for path in recent.iter().take(5) {
        let name = path.rsplit(['/', '\\']).find(|p| !p.is_empty()).unwrap_or(path).to_string();
        v.push((Icon::History, name, path.clone()));
    }
    v
}

/// Draw the explorer (address, places, folders) into `ui`. `list_h` is the height of its list of folders.
fn explorer_ui(ui: &mut egui::Ui, pal: &Palette, ex: &mut Explorer, list_h: f32, recent: &[String]) -> Outcome {
    let mut outcome = Outcome::None;
    let ctx = ui.ctx().clone();
    let address_id = egui::Id::unique(("explorer_address", ex.side == Side::Linux));
    let search_id = egui::Id::unique(("explorer_search", ex.side == Side::Linux));
    let mut navigate: Option<String> = None;
    let busy = ex.request.is_some() || ex.changed_at.is_some();
    let typing = ctx.memory(|m| m.has_focus(address_id)) || (ctx.memory(|m| m.has_focus(search_id)) && !ex.query.is_empty());
    let visible: Vec<Item> = ex.visible().into_iter().cloned().collect();
    let folders: Vec<&Item> = visible.iter().filter(|i| i.is_dir).collect();

    // the keyboard: arrows choose a folder, Enter goes into it, Backspace up, Ctrl+Enter opens it
    let mut moved = false;
    if !busy && !ctx.memory(|m| m.has_focus(address_id)) {
        let (down, up) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, Key::ArrowUp)));
        if (down || up) && !folders.is_empty() {
            let current = ex.selected.as_ref().and_then(|s| folders.iter().position(|f| f.path == *s));
            let next = match current {
                Some(i) if down => (i + 1).min(folders.len() - 1),
                Some(i) => i.saturating_sub(1),
                None if down => 0,
                None => folders.len() - 1,
            };
            ex.selected = Some(folders[next].path.clone());
            moved = true;
        }
        if ex.error.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, Key::Enter)) {
            if let Some(t) = ex.target() {
                outcome = Outcome::Choose(t.clone());
            }
        } else if ex.selected.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
            navigate = ex.selected.as_ref().map(|p| display_path(p));
        }
        if !typing && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Backspace))
            && let Some(parent) = ex.current.as_ref().and_then(|p| p.parent())
        {
            navigate = Some(display_path(parent));
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, Key::ArrowLeft)) {
            ex.go_back();
        }
    }

    // ---- address: back, up, the steps of the path (or a field to type one), refresh
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let can_back = !ex.back.is_empty();
        if ui.add_enabled_ui(can_back, |ui| widgets::icon_button(ui, pal, Icon::ArrowLeft, 32.0, pal.text_dim, tr("Back (Alt+Left)"))).inner.clicked() {
            ex.go_back();
        }
        let parent = ex.current.as_ref().and_then(|p| p.parent()).map(display_path);
        if ui.add_enabled_ui(parent.is_some(), |ui| widgets::icon_button(ui, pal, Icon::ArrowUp, 32.0, pal.text_dim, tr("Up one folder (Backspace)"))).inner.clicked() {
            navigate = parent;
        }
        let w = (ui.available_width() - 40.0).max(120.0);
        if ex.editing {
            let response = TextField::new(&mut ex.path).id(address_id).hint(if ex.side == Side::Linux { "~/projects" } else { "C:\\Projects" }).width(w).show(ui, pal);
            if !response.has_focus() && !response.lost_focus() {
                response.request_focus();
            }
            if response.lost_focus() {
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    navigate = Some(ex.path.clone());
                } else {
                    ex.editing = false;
                    ex.path = ex.current.as_deref().map(display_path).unwrap_or_default();
                }
            }
        } else {
            // the address as steps: a click goes to one, a click beside them types a path
            let (bar, bar_resp) = ui.allocate_exact_size(Vec2::new(w, widgets::FIELD_H), Sense::click());
            ui.painter().rect_filled(bar, CornerRadius::same(12), pal.input_bg);
            let mut x = bar.min.x + 10.0;
            let shown = ex.current.as_deref().map(display_path).unwrap_or_else(|| ex.path.clone());
            let steps = crumbs(ex.side, &shown);
            // the last steps that fit: the start of a long path gives way to an ellipsis
            let widths: Vec<f32> = steps.iter().map(|(name, _)| ui.painter().layout_no_wrap(name.clone(), fonts::ui(13.0), pal.text).size().x + 16.0).collect();
            let mut first = 0;
            while first + 1 < steps.len() && widths[first..].iter().sum::<f32>() + 30.0 > bar.width() - 20.0 {
                first += 1;
            }
            if first > 0 {
                ui.painter().text(Pos2::new(x, bar.center().y), Align2::LEFT_CENTER, "…", fonts::ui(13.0), pal.text_dim);
                x += 18.0;
            }
            let last = steps.len().saturating_sub(1);
            for (i, (name, target)) in steps.iter().enumerate().skip(first) {
                let r = Rect::from_min_size(Pos2::new(x, bar.min.y + 4.0), Vec2::new(widths[i], bar.height() - 8.0));
                let resp = ui.interact(r, address_id.with(("crumb", i)), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(8), pal.hover.gamma_multiply(1.6));
                }
                ui.painter().text(r.center(), Align2::CENTER_CENTER, name, if i == last { fonts::semibold(13.0) } else { fonts::ui(13.0) }, if i == last { pal.text } else { pal.text_dim });
                if resp.clicked() && i != last {
                    navigate = Some(target.clone());
                }
                x += widths[i];
                if i != last {
                    icons::paint(ui.painter(), Rect::from_center_size(Pos2::new(x + 1.0, bar.center().y), Vec2::splat(10.0)), Icon::ChevronRight, pal.text_dim);
                    x += 4.0;
                }
            }
            let pencil = Rect::from_center_size(Pos2::new(bar.max.x - 18.0, bar.center().y), Vec2::splat(14.0));
            icons::paint(ui.painter(), pencil, Icon::Pencil, pal.text_dim);
            if bar_resp.on_hover_text(tr("Click to type a path")).clicked() {
                ex.editing = true;
                ex.path = shown;
            }
        }
        if widgets::icon_button(ui, pal, Icon::Refresh, 32.0, pal.text_dim, tr("Refresh")).clicked() {
            ex.start();
        }
    });
    ui.add_space(8.0);

    // ---- places | folders
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.vertical(|ui| {
            ui.set_width(170.0);
            let current = ex.current.as_deref().map(display_path);
            let mut last_kind_recent = false;
            for (icon, name, target) in places(ex.side, recent) {
                let is_recent = icon == Icon::History;
                if is_recent && !last_kind_recent {
                    ui.add_space(6.0);
                    widgets::caption(ui, pal, &tr("Recent projects").to_uppercase());
                    ui.add_space(2.0);
                }
                last_kind_recent = is_recent;
                let (r, resp) = ui.allocate_exact_size(Vec2::new(170.0, 30.0), Sense::click());
                let here = current.as_deref() == Some(target.as_str()) || (target == "~" && current.as_deref().is_some_and(|c| c.starts_with("/home/") && c.matches('/').count() == 2));
                widgets::row_background(ui, pal, r.expand2(Vec2::new(8.0, 0.0)), here, resp.hovered());
                icons::paint(ui.painter(), Rect::from_center_size(Pos2::new(r.min.x + 14.0, r.center().y), Vec2::splat(15.0)), icon, if here { pal.accent } else { pal.text_dim });
                ui.painter().with_clip_rect(r).text(Pos2::new(r.min.x + 32.0, r.center().y), Align2::LEFT_CENTER, &name, fonts::ui(13.0), if here { pal.text } else { pal.text_dim });
                let resp = resp.on_hover_text(&target);
                if resp.clicked() {
                    navigate = Some(target.clone());
                }
            }
        });
        ui.vertical(|ui| {
            let response = TextField::new(&mut ex.query).id(search_id).hint(tr("Filter by name...")).icon(Icon::Search).show(ui, pal);
            widgets::focus_once(ui, &response, &mut ex.just_opened);
            let mut changed = false;
            ui.horizontal(|ui| {
                changed |= widgets::checkbox(ui, pal, &mut ex.recursive, tr("Search subfolders")).changed();
                ui.add_space(8.0);
                changed |= widgets::checkbox(ui, pal, &mut ex.hidden, tr("Show hidden")).changed();
            });
            // a filter narrows what is listed at once; a search of subfolders asks the disk again
            if changed || (response.changed() && (ex.recursive || ex.found)) {
                ex.request = None;
                ex.changed_at = Some(Instant::now());
                ex.selected = None;
            }
            ui.add_space(4.0);
            let (frame_rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), list_h), Sense::hover());
            ui.painter().rect_filled(frame_rect, CornerRadius::same(12), pal.input_bg);
            let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(frame_rect.shrink(4.0)).layout(egui::Layout::top_down(egui::Align::Min)));
            egui::ScrollArea::vertical().id_salt(("explorer_list", ex.side == Side::Linux)).auto_shrink([false, false]).show_fading(&mut list_ui, |ui| {
                let now = SystemTime::now();
                let offset = crate::file_info::local_offset();
                if busy {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.spinner();
                        ui.label(RichText::new(if ex.found || (ex.recursive && !ex.query.is_empty()) { tr("Looking for folders...") } else { tr("Reading the folder...") }).color(pal.text_dim));
                    });
                    return;
                }
                if let Some(error) = &ex.error {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(8.0);
                        ui.colored_label(pal.error, error);
                    });
                    return;
                }
                if visible.is_empty() {
                    ui.add_space(14.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new(if ex.query.is_empty() { tr("This folder is empty") } else { tr("Nothing matches") }).color(pal.text_dim)));
                    return;
                }
                for item in &visible {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), if item.is_dir { Sense::click() } else { Sense::hover() });
                    let chosen = ex.selected.as_ref() == Some(&item.path);
                    if item.is_dir {
                        widgets::row_background(ui, pal, r.expand2(Vec2::new(6.0, 0.0)), chosen, resp.hovered());
                    }
                    let icon = if item.project { Icon::FolderGit } else if item.is_dir { Icon::Folder } else { Icon::Blank };
                    let color = if !item.is_dir { pal.text_dim.gamma_multiply(0.7) } else if item.project { pal.accent } else { pal.builtin.gamma_multiply(0.85) };
                    icons::paint(ui.painter(), Rect::from_center_size(Pos2::new(r.min.x + 16.0, r.center().y), Vec2::splat(16.0)), icon, color);
                    let name_color = if item.is_dir { pal.text } else { pal.text_dim.gamma_multiply(0.8) };
                    let mut right = r.max.x - 10.0;
                    if let Some(time) = item.modified {
                        let text = if item.is_dir { crate::file_info::modified(time, now, offset) } else { format!("{}  ·  {}", crate::file_info::size(item.size), crate::file_info::modified(time, now, offset)) };
                        let g = ui.painter().layout_no_wrap(text, fonts::ui(11.5), pal.text_dim);
                        right -= g.size().x;
                        ui.painter().galley(Pos2::new(right, r.center().y - g.size().y / 2.0), g, pal.text_dim);
                        right -= 12.0;
                    }
                    if item.project {
                        let g = ui.painter().layout_no_wrap(tr("project").to_string(), fonts::semibold(10.5), pal.accent);
                        let chip = Rect::from_min_size(Pos2::new(right - g.size().x - 12.0, r.center().y - 9.0), Vec2::new(g.size().x + 12.0, 18.0));
                        ui.painter().rect_filled(chip, CornerRadius::same(6), pal.accent.gamma_multiply(0.16));
                        ui.painter().galley(chip.center() - g.size() / 2.0, g, pal.accent);
                        right = chip.min.x - 8.0;
                    }
                    let name_rect = Rect::from_min_max(Pos2::new(r.min.x + 32.0, r.min.y), Pos2::new(right.max(r.min.x + 60.0), r.max.y));
                    ui.painter().with_clip_rect(name_rect.intersect(ui.clip_rect())).text(Pos2::new(name_rect.min.x, r.center().y), Align2::LEFT_CENTER, &item.name, fonts::ui(13.5), name_color);
                    if item.is_dir {
                        if resp.clicked() {
                            ex.selected = Some(item.path.clone());
                        }
                        if resp.double_clicked() {
                            navigate = Some(display_path(&item.path));
                        }
                        if chosen && moved {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                        resp.on_hover_text(display_path(&item.path));
                    }
                }
            });
            let note = if ex.limited {
                Some(tr("Not everything is shown. Open a closer folder or refine the name."))
            } else if ex.skipped > 0 {
                Some(tr("Some folders could not be read."))
            } else if ex.recursive && !ex.query.is_empty() {
                Some(tr("Searches up to 8 levels; skips dependency and build folders."))
            } else {
                None
            };
            if let Some(note) = note {
                ui.label(RichText::new(note).color(pal.text_dim).size(11.5));
            }
        });
    });
    if let Some(path) = navigate {
        ex.navigate(path);
    }
    if outcome_is_none(&outcome) && ex.changed_at.is_some_and(|t| t.elapsed() >= Duration::from_millis(300)) {
        ex.start();
    }
    if ex.request.is_some() || ex.changed_at.is_some() {
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    outcome
}

fn outcome_is_none(o: &Outcome) -> bool {
    matches!(o, Outcome::None)
}

/// The footer of an explorer's dialog: where it points, and its buttons. `native`: offer the system's folder dialog.
fn explorer_footer(ui: &mut egui::Ui, pal: &Palette, ex: &Explorer, native: bool) -> Outcome {
    let mut outcome = Outcome::None;
    ui.add_space(10.0);
    let target = ex.target().cloned();
    widgets::right_row(ui, 38.0, |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let label = if ex.purpose == Purpose::Open { tr("Open Project") } else { tr("Choose This Folder") };
        let enabled = ex.idle() && ex.error.is_none() && target.is_some();
        if ui.add_enabled_ui(enabled, |ui| widgets::primary_button(ui, pal, label)).inner.on_hover_text("Ctrl+Enter").clicked()
            && let Some(t) = &target
        {
            outcome = Outcome::Choose(t.clone());
        }
        if widgets::secondary_button(ui, pal, tr("Cancel")).clicked() {
            outcome = Outcome::Close;
        }
        if native && widgets::secondary_button(ui, pal, tr("System dialog...")).on_hover_text(tr("Choose the folder in the system's own window")).clicked() {
            outcome = Outcome::Native;
        }
        if let Some(t) = &target {
            let shown = display_path(t);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                icons::paint(ui.painter(), Rect::from_center_size(ui.cursor().min + Vec2::new(8.0, 19.0), Vec2::splat(14.0)), Icon::Folder, pal.text_dim);
                ui.add_space(22.0);
                ui.add(egui::Label::new(RichText::new(shown).color(pal.text_dim).size(12.5)).truncate());
            });
        }
    });
    outcome
}

/// The state of the Open Project dialog: an explorer for each side, and the side shown.
pub struct ProjectBrowser {
    pub side: Side,
    pub local: Explorer,
    pub linux: Explorer,
}

impl ProjectBrowser {
    pub fn idle(&self) -> bool {
        self.shown().idle()
    }

    pub fn shown(&self) -> &Explorer {
        match self.side {
            Side::Local => &self.local,
            Side::Linux => &self.linux,
        }
    }

    pub fn shown_mut(&mut self) -> &mut Explorer {
        match self.side {
            Side::Local => &mut self.local,
            Side::Linux => &mut self.linux,
        }
    }

    /// Show the other side; its folder is read the first time it is shown.
    pub fn switch(&mut self, side: Side) {
        self.side = side;
        let ex = self.shown_mut();
        if ex.current.is_none() && ex.idle() && ex.error.is_none() {
            ex.start();
        }
    }
}

/// A window with a folder explorer over everything else (`id` keeps it apart from other windows): its outcome.
pub fn explorer_window(ctx: &egui::Context, pal: &Palette, id: &str, title: &str, ex: &mut Explorer, distros: &[String], probing: bool) -> Outcome {
    ex.poll();
    if ex.side == Side::Linux && ex.distro.is_empty() && let Some(d) = distros.first() {
        ex.distro = d.clone();
        ex.start();
    }
    // Esc leaves the typing of a path first, then closes
    let mut outcome = Outcome::None;
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        if ex.editing {
            ex.editing = false;
        } else {
            outcome = Outcome::Close;
        }
    }
    let screen = ctx.content_rect();
    let width = (screen.width() - 64.0).clamp(320.0, 820.0);
    let list_h = (screen.height() - 330.0).clamp(120.0, 420.0);
    let area = egui::Area::new(egui::Id::unique(("explorer_window", id))).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
        widgets::glass_frame(pal).shadow(egui::Shadow { offset: [0, 12], blur: 36, spread: 0, color: egui::Color32::from_black_alpha(if pal.dark { 120 } else { 40 }) }).show(ui, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).font(fonts::semibold(17.0)));
                if ex.side == Side::Linux && cfg!(windows) {
                    ui.add_space(8.0);
                    distro_choice(ui, pal, ex, distros, probing);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, pal, Icon::Close, 28.0, pal.text_dim, tr("Close (Esc)")).clicked() {
                        outcome = Outcome::Close;
                    }
                });
            });
            ui.add_space(10.0);
            if ex.side == Side::Linux && cfg!(windows) && ex.distro.is_empty() {
                no_distro(ui, pal, probing, list_h, &mut outcome);
            } else {
                let o = explorer_ui(ui, pal, ex, list_h, &[]);
                if !outcome_is_none(&o) {
                    outcome = o;
                }
            }
            let o = explorer_footer(ui, pal, ex, false);
            if !outcome_is_none(&o) {
                outcome = o;
            }
        });
    });
    ctx.move_to_top(area.response.layer_id);
    outcome
}

/// The distribution an explorer of Linux shows, as a dropdown.
fn distro_choice(ui: &mut egui::Ui, pal: &Palette, ex: &mut Explorer, distros: &[String], probing: bool) {
    if distros.is_empty() {
        if probing {
            ui.spinner();
        }
        return;
    }
    let shown = ex.distro.clone();
    let mut chosen = None;
    widgets::dropdown(ui, pal, &format!("explorer_distro_{:?}", ex.purpose), 200.0, &shown, |ui| {
        for d in distros {
            if widgets::menu_row(ui, pal, Some(if *d == ex.distro { Icon::Check } else { Icon::Linux }), d, "").clicked() {
                chosen = Some(d.clone());
                ui.close();
            }
        }
    });
    if let Some(d) = chosen.filter(|d| *d != ex.distro) {
        ex.distro = d;
        ex.back.clear();
        ex.current = None;
        ex.navigate("~".into());
    }
}

/// In place of the explorer of Linux when there is no distribution: as tall as the explorer, so nothing jumps.
fn no_distro(ui: &mut egui::Ui, pal: &Palette, probing: bool, list_h: f32, outcome: &mut Outcome) {
    let top = ui.cursor().min.y;
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        if probing {
            ui.spinner();
            ui.label(RichText::new(tr("Looking for Linux distributions...")).color(pal.text_dim));
        } else {
            icons::paint(ui.painter(), Rect::from_center_size(ui.cursor().center_top() + Vec2::new(0.0, 16.0), Vec2::splat(30.0)), Icon::Linux, pal.text_dim);
            ui.add_space(38.0);
            ui.label(RichText::new(tr("No Linux distribution is installed yet")).color(pal.text_dim));
            ui.add_space(10.0);
            ui.label(RichText::new(tr("Projects can live in Linux once WSL and a distribution are set up.")).color(pal.text_dim).size(12.0));
            ui.add_space(10.0);
            if widgets::primary_button(ui, pal, tr("Linux (WSL) setup...")).clicked() {
                *outcome = Outcome::Setup;
            }
        }
    });
    let used = ui.cursor().min.y - top;
    ui.add_space((list_h + 96.0 - used).max(30.0));
}

impl App {
    pub fn open_project_browser(&mut self) {
        if cfg!(windows) && self.wsl.probe.is_none() && !self.wsl.probing() {
            self.wsl_probe();
        }
        let distro = self.suggested_distro().unwrap_or_default();
        // this computer comes first; a project open in Linux opens the dialog on Linux, where it is
        let (mut side, mut local_path, mut linux_path) = (Side::Local, crate::input::dirs_home().display().to_string(), "~".to_string());
        let mut linux = Explorer::new(Side::Linux, Purpose::Open, distro, String::new());
        if let Some(root) = self.project_root() {
            match ide_core::wsl::wsl_path(root) {
                Some(path) if cfg!(windows) => {
                    side = Side::Linux;
                    linux.distro = path.distro;
                    linux_path = root.parent().and_then(ide_core::wsl::wsl_path).map_or(path.linux, |p| p.linux);
                }
                _ => local_path = root.parent().unwrap_or(root).display().to_string(),
            }
        }
        linux.path = linux_path;
        let mut browser = ProjectBrowser { side, local: Explorer::new(Side::Local, Purpose::Open, String::new(), local_path), linux };
        browser.shown_mut().start();
        self.popup = Popup::ProjectBrowser(Box::new(browser));
    }

    pub fn draw_project_browser(&mut self, ctx: &egui::Context) {
        if !matches!(self.popup, Popup::ProjectBrowser(_)) {
            return;
        }
        let Popup::ProjectBrowser(mut st) = std::mem::replace(&mut self.popup, Popup::None) else {
            return;
        };
        st.local.poll();
        st.linux.poll();
        let distros = self.wsl.distros();
        if cfg!(windows) && st.linux.distro.is_empty() && !distros.is_empty() {
            st.linux.distro = self.suggested_distro().unwrap_or_else(|| distros[0].clone());
            if st.side == Side::Linux {
                st.linux.start();
            }
        }
        let pal = self.pal;
        let probing = self.wsl.probing();
        let side = st.side;
        let recent: Vec<String> = self.settings.recent_projects.iter().filter(|p| (ide_core::wsl::wsl_path(Path::new(p)).is_some()) == (side == Side::Linux)).filter_map(|p| match side {
            Side::Linux => ide_core::wsl::wsl_path(Path::new(p)).filter(|w| w.distro == st.linux.distro).map(|w| w.linux),
            Side::Local => Some(p.clone()),
        }).collect();
        // Esc leaves the typing of a path first, then closes
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        let mut outcome = if esc && !st.shown().editing { Outcome::Close } else { Outcome::None };
        if esc && st.shown().editing {
            st.shown_mut().editing = false;
        }
        let mut switch = None;
        let screen = ctx.content_rect();
        let width = (screen.width() - 64.0).clamp(320.0, 860.0);
        let list_h = (screen.height() - 360.0).clamp(110.0, 440.0);
        egui::Area::new(egui::Id::unique("project_browser")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            widgets::glass_frame(&pal).show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Open Project")).font(fonts::semibold(17.0)));
                    ui.add_space(12.0);
                    // where the project is: this computer first, Linux beside it
                    if cfg!(windows) {
                        let labels = [tr("This computer"), tr("Linux (WSL)")];
                        if let Some(i) = widgets::segmented(ui, &pal, &labels, usize::from(side == Side::Linux)) {
                            switch = Some(if i == 0 { Side::Local } else { Side::Linux });
                        }
                        if side == Side::Linux {
                            ui.add_space(4.0);
                            distro_choice(ui, &pal, &mut st.linux, &distros, probing);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, &pal, Icon::Close, 28.0, pal.text_dim, tr("Close (Esc)")).clicked() {
                            outcome = Outcome::Close;
                        }
                        if side == Side::Linux && cfg!(windows) && !st.linux.distro.is_empty() && widgets::secondary_button(ui, &pal, tr("Linux (WSL) setup...")).clicked() {
                            outcome = Outcome::Setup;
                        }
                    });
                });
                ui.add_space(12.0);
                let ex = st.shown_mut();
                if side == Side::Linux && cfg!(windows) && ex.distro.is_empty() {
                    no_distro(ui, &pal, probing, list_h, &mut outcome);
                } else {
                    let o = explorer_ui(ui, &pal, ex, list_h, &recent);
                    if !outcome_is_none(&o) {
                        outcome = o;
                    }
                }
                let o = explorer_footer(ui, &pal, st.shown(), side == Side::Local);
                if !outcome_is_none(&o) {
                    outcome = o;
                }
            });
        });
        if let Some(side) = switch {
            st.switch(side);
        }
        match outcome {
            Outcome::Close => {}
            Outcome::Native => {
                let mut dialog = rfd::FileDialog::new().set_title(tr("Open Project"));
                if let Some(dir) = st.local.current.clone() {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(folder) = dialog.pick_folder() {
                    self.request_open_project(&folder);
                } else {
                    self.popup = Popup::ProjectBrowser(st);
                }
            }
            Outcome::Setup => self.open_settings_page(crate::ui_settings::WSL_PAGE),
            Outcome::Choose(path) => self.request_open_project(&path),
            Outcome::None => {
                if probing {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
                self.popup = Popup::ProjectBrowser(st);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!("light-browser-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn folder(&self, name: &str) {
            std::fs::create_dir_all(self.0.join(name)).unwrap();
        }
        fn scan(&self, query: &str, hidden: bool, recursive: bool) -> Vec<PathBuf> {
            scan_folders(self.0.clone(), query, hidden, recursive, &AtomicBool::new(false)).unwrap().items.into_iter().map(|i| i.path).collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let path = self.0.canonicalize().unwrap();
            assert_eq!(path.parent(), Some(std::env::temp_dir().canonicalize().unwrap().as_path()));
            assert!(path.file_name().unwrap().to_string_lossy().starts_with("light-browser-test-"));
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn a_folder_lists_its_folders_then_its_files_and_marks_projects() {
        let f = Fixture::new();
        for name in ["Zoo/deep", "alpha/.git", ".hidden"] {
            f.folder(name);
        }
        std::fs::write(f.0.join("notes.txt"), "four").unwrap();
        let listing = list_folder(f.0.clone(), false, &AtomicBool::new(false)).unwrap();
        let names: Vec<(&str, bool, bool)> = listing.items.iter().map(|i| (i.name.as_str(), i.is_dir, i.project)).collect();
        assert_eq!(names, [("alpha", true, true), ("Zoo", true, false), ("notes.txt", false, false)]);
        assert_eq!(listing.items[2].size, 4);
        assert!(listing.items[2].modified.is_some());
        assert_eq!(list_folder(f.0.clone(), true, &AtomicBool::new(false)).unwrap().items.len(), 4);
        assert!(list_folder(f.0.join("missing"), false, &AtomicBool::new(false)).is_err());
        assert!(list_folder(f.0.clone(), false, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn recursive_search_matches_names_and_skips_dependencies() {
        let f = Fixture::new();
        for name in ["work/My Project", "node_modules/My Project", "target/My Project", ".hidden/My Project"] {
            f.folder(name);
        }
        assert_eq!(f.scan("my project", false, true), [f.0.join("work/My Project")]);
        assert!(f.scan("my project", false, false).is_empty());
        assert_eq!(f.scan("MY PROJECT", true, true).len(), 2);
        // Dependency folders can still be opened directly.
        assert_eq!(f.scan("node_modules", false, true), [f.0.join("node_modules")]);
        // the found folders are named by their place below the folder searched
        let found = scan_folders(f.0.clone(), "my project", false, true, &AtomicBool::new(false)).unwrap();
        assert_eq!(found.items[0].name, "work/My Project");
        assert!(found.found);
    }

    #[test]
    fn limits_cancellation_and_missing_directory_are_reported() {
        let f = Fixture::new();
        for index in 0..MAX_RESULTS + 2 {
            f.folder(&format!("project-{index}"));
        }
        let found = scan_folders(f.0.clone(), "", false, false, &AtomicBool::new(false)).unwrap();
        assert_eq!(found.items.len(), MAX_RESULTS);
        assert!(found.limited);
        assert!(list_folder(f.0.clone(), false, &AtomicBool::new(false)).unwrap().limited);
        assert!(scan_folders(f.0.clone(), "", false, false, &AtomicBool::new(true)).is_err());
        assert!(scan_folders(f.0.join("missing"), "", false, false, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn linux_paths_are_normalized_without_interpreting_shell_characters() {
        let resolved = resolve_folder("Ubuntu", "/home/me/../some project/./$(echo hi)").unwrap();
        assert_eq!(display_path(&resolved), "/home/some project/$(echo hi)");
        assert_eq!(display_path(&resolve_folder("Ubuntu", "/../../").unwrap()), "/");
        assert!(resolve_folder("Ubuntu", "relative/project").is_err());
    }

    #[test]
    fn local_paths_must_be_full_and_tilde_is_home() {
        assert_eq!(resolve_local("~").unwrap(), crate::input::dirs_home());
        assert_eq!(resolve_local("~/work").unwrap(), crate::input::dirs_home().join("work"));
        assert!(resolve_local("relative\\dir").is_err());
        if cfg!(windows) {
            assert_eq!(resolve_local("E:").unwrap(), PathBuf::from("E:\\"));
            assert_eq!(resolve_local("\"C:\\Program Files\"").unwrap(), PathBuf::from("C:\\Program Files"));
        }
    }

    #[test]
    fn the_address_is_cut_into_steps() {
        let steps = |side, s: &str| crumbs(side, s).into_iter().map(|(name, path)| format!("{name}={path}")).collect::<Vec<_>>();
        assert_eq!(steps(Side::Local, "E:\\Projects\\Cds"), ["E:=E:\\", "Projects=E:\\Projects", "Cds=E:\\Projects\\Cds"]);
        assert_eq!(steps(Side::Local, "E:\\"), ["E:=E:\\"]);
        assert_eq!(steps(Side::Linux, "/home/me"), ["/=/", "home=/home", "me=/home/me"]);
        assert_eq!(steps(Side::Linux, "/"), ["/=/"]);
    }

    #[test]
    fn this_computer_has_its_drives_and_home_among_the_places() {
        let local = places(Side::Local, &["E:\\work\\demo".to_string()]);
        assert_eq!(local[0].0, Icon::Home);
        assert!(local.iter().any(|p| p.0 == Icon::Drive));
        assert_eq!(local.last().map(|p| (p.0, p.1.as_str())), Some((Icon::History, "demo")));
        let linux = places(Side::Linux, &[]);
        assert_eq!(linux.iter().map(|p| p.2.as_str()).collect::<Vec<_>>(), ["~", "/", "/mnt"]);
    }

    #[test]
    fn the_dialog_shows_this_computer_first_with_its_controls_on_screen() {
        for (width, height) in [(1280.0, 800.0), (640.0, 400.0)] {
            let ctx = egui::Context::default();
            fonts::install(&ctx);
            let mut app = App::new(ide_core::settings::Settings::default());
            let f = Fixture::new();
            f.folder("project/.git");
            let mut local = Explorer::new(Side::Local, Purpose::Open, String::new(), f.0.display().to_string());
            let listing = list_folder(f.0.clone(), false, &AtomicBool::new(false)).unwrap();
            local.current = Some(listing.root);
            local.items = listing.items;
            local.path = f.0.display().to_string();
            app.popup = Popup::ProjectBrowser(Box::new(ProjectBrowser { side: Side::Local, local, linux: Explorer::new(Side::Linux, Purpose::Open, "Ubuntu".into(), "~".into()) }));
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
            for frame in 0..4 {
                let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                    egui::Panel::top("browser_test_toolbar").exact_size(86.0).show(ui, |_| {});
                    app.draw_project_browser(&ctx);
                });
                if frame == 3 {
                    let mut wanted = vec!["Open Project", "Cancel", "project"];
                    if cfg!(windows) {
                        wanted.push("This computer");
                    }
                    for wanted in wanted {
                        let text = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if text.galley.job.text == wanted => Some((shape.clip_rect, egui::Rect::from_min_size(text.pos, text.galley.size()))),
                                _ => None,
                            })
                            .unwrap_or_else(|| panic!("missing browser control: {wanted} at {width}x{height}"));
                        assert!(text.0.contains_rect(text.1), "browser control clipped: {wanted}: {text:?}");
                        assert!(screen.contains_rect(text.1), "browser control outside viewport: {wanted}");
                    }
                }
                output.textures_delta.clear();
            }
            assert!(matches!(&app.popup, Popup::ProjectBrowser(b) if b.side == Side::Local));
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_folders_are_selectable_without_following_cycles() {
        let f = Fixture::new();
        f.folder("work/project");
        std::os::unix::fs::symlink(&f.0, f.0.join("work/loop")).unwrap();
        assert_eq!(f.scan("loop", false, true), [f.0.join("work/loop")]);
    }
}

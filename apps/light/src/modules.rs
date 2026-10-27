//! The modules of the IDE: what is installed, what the registry offers, and the work of downloading and removing them
//! (on background threads, collected every frame by [`App::poll_modules`]). What a module means for the editor is in
//! [`LanguageSet`](light_lang::LanguageSet): the languages of the enabled modules, rebuilt whenever the set changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use light_lang::{Language, LanguageSet};
use eframe::egui::{self, TextureHandle, TextureOptions};
use ide_core::modules::registry::FileEntry;
use ide_core::modules::{Detect, Index, IndexEntry, Installed, Manifest, ModuleStore, Source, default_sources, fetch_index, is_newer};
use lisp_syntax::dialect::is_lisp_file;

use crate::app::App;
use crate::doc::Doc;
use crate::editor_view::Sev;
use crate::state::BottomTool;
use crate::i18n::{self, tr};

/// The Lisp module is part of the program (its engine is compiled in), so its files come with it: a person who used
/// Light for Lisp before modules existed has it from the first start, offline.
const LISP_MODULE_JSON: &[u8] = include_bytes!("../../../registry/modules/lisp/module.json");
const LISP_MODULE_COVER: &[u8] = include_bytes!("../../../registry/modules/lisp/cover.png");
const LISP_MODULE_LICENSE: &[u8] = include_bytes!("../../../registry/modules/lisp/LICENSE");
const LISP_MODULE_NOTICES: &[u8] = include_bytes!("../../../registry/modules/lisp/THIRD_PARTY_NOTICES.md");

/// What a module is doing right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Busy {
    pub removing: bool,
    pub done: usize,
    pub total: usize,
    pub file: String,
}

enum Event {
    Index(Result<(Index, Source), String>),
    /// A cover picture is in the cache.
    Cover(String),
    /// All the covers have been fetched.
    CoversDone,
    Progress { id: String, done: usize, total: usize, file: String },
    /// A module was installed or removed (boxed: a manifest is much bigger than the other events).
    Finished { id: String, removed: bool, result: Box<Result<Manifest, String>> },
}

pub struct ModulesState {
    pub store: Option<ModuleStore>,
    pub installed: Vec<Installed>,
    /// Folders of installed modules that cannot be used, and why.
    pub broken: Vec<String>,
    pub langs: LanguageSet,
    pub lisp: bool,
    pub index: Option<Index>,
    pub index_source: Option<Source>,
    pub index_error: Option<String>,
    pub loading_index: bool,
    /// The cover pictures are still being fetched.
    pub loading_covers: bool,
    pub busy: HashMap<String, Busy>,
    /// Pictures already turned into textures.
    pub covers: HashMap<String, TextureHandle>,
    pub filter: String,
    /// The filter chip: `None` all, `@installed`, or a category.
    pub category: Option<String>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl Default for ModulesState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        ModulesState { store: None, installed: vec![], broken: vec![], langs: LanguageSet::new(), lisp: false, index: None, index_source: None, index_error: None, loading_index: false, loading_covers: false, busy: HashMap::new(), covers: HashMap::new(), filter: String::new(), category: None, tx, rx }
    }
}

/// A module as the Modules tab shows it.
#[derive(Clone, Debug)]
pub struct Card {
    pub id: String,
    pub name: String,
    pub category: String,
    pub summary: String,
    /// The version on offer (from the registry) or installed.
    pub version: String,
    pub size: u64,
    pub installed: Option<Installed>,
    /// A newer version than the installed one is on offer.
    pub update: Option<String>,
    pub detect: Detect,
}

/// The registry that is shipped next to the program (and, when running from the sources, the one in the repository).
pub fn bundled_registry() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("registry")));
    #[cfg(target_os = "macos")]
    let beside = std::env::current_exe().ok().and_then(|e| {
        let directory = e.parent()?;
        if directory.file_name().is_some_and(|name| name == "MacOS") && directory.parent()?.file_name().is_some_and(|name| name == "Contents") {
            Some(directory.parent()?.join("Resources/registry"))
        } else {
            beside
        }
    });
    let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../registry");
    [beside, Some(sources)].into_iter().flatten().find(|p| p.join("index.json").is_file())
}

impl ModulesState {
    pub fn cache_dir(&self) -> Option<PathBuf> {
        self.store.as_ref().map(|s| s.dir().join(".covers"))
    }

    /// Artwork follows the current catalog, independently of the installed code's version.
    pub fn cover_file(&self, id: &str) -> Option<PathBuf> {
        if let Some(e) = self.index.as_ref().and_then(|i| i.entry(id))
            && let Some(f) = cover_entry(e)
            && let Some(cache) = self.cache_dir()
        {
            let p = cache.join(cache_name(id, f));
            if p.is_file() {
                return Some(p);
            }
        }
        // Updated bundled artwork also works offline with an older installed module.
        if let Some(registry) = bundled_registry()
            && let Ok(json) = std::fs::read_to_string(registry.join("index.json"))
            && let Ok(index) = Index::parse(&json)
            && let Some(entry) = index.entry(id)
        {
            let p = registry.join(&entry.path).join(&entry.cover);
            if p.is_file() {
                return Some(p);
            }
        }
        self.installed.iter().find(|m| m.manifest.id == id).and_then(Installed::cover_path)
    }

    /// The cards of the Modules tab: installed modules and everything the registry offers, by name.
    pub fn cards(&self) -> Vec<Card> {
        let mut cards: Vec<Card> = Vec::new();
        for m in &self.installed {
            let entry = self.index.as_ref().and_then(|i| i.entry(&m.manifest.id));
            let update = entry.filter(|e| is_newer(&e.version, &m.manifest.version)).map(|e| e.version.clone());
            cards.push(Card {
                id: m.manifest.id.clone(),
                name: m.manifest.name.clone(),
                category: m.manifest.category.clone(),
                summary: m.manifest.summary.clone(),
                version: m.manifest.version.clone(),
                size: entry.map_or(0, |e| e.size()),
                installed: Some(m.clone()),
                update,
                detect: m.manifest.detect.clone(),
            });
        }
        if let Some(i) = &self.index {
            let known: Vec<String> = cards.iter().map(|c| c.id.clone()).collect();
            for e in i.modules.iter().filter(|e| !known.contains(&e.id)) {
                cards.push(Card { id: e.id.clone(), name: e.name.clone(), category: e.category.clone(), summary: e.summary.clone(), version: e.version.clone(), size: e.size(), installed: None, update: None, detect: e.detect.clone() });
            }
        }
        cards.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        cards
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.installed.iter().any(|m| m.manifest.id == id && m.enabled)
    }

    /// `(id, how to detect it)` of every module that is known, installed or on offer.
    pub fn detectors(&self) -> Vec<(String, Detect)> {
        self.cards().into_iter().map(|c| (c.id, c.detect)).collect()
    }
}

fn cache_name(id: &str, f: &FileEntry) -> String {
    format!("{id}-{}.png", f.sha256.chars().take(12).collect::<String>())
}

impl App {
    // ------------------------------------------------------------------ start

    /// Open the module store, make the first-run choice and load what is installed.
    pub fn modules_init(&mut self) {
        let Some(dir) = ModuleStore::default_dir() else { return };
        self.mods.store = Some(ModuleStore::at(dir.clone()));
        let store = ModuleStore::at(dir);
        if !store.has_state() {
            // The first start with modules. Whoever used the IDE before gets Lisp, which was all it did; a new user
            // is asked what they work with.
            let used_before = !self.settings.recent_projects.is_empty() || !self.settings.open_files.is_empty();
            let mut state = store.state();
            state.seeded = true;
            if used_before {
                match store.install_bytes("lisp", &[("module.json", LISP_MODULE_JSON), ("cover.png", LISP_MODULE_COVER), ("LICENSE", LISP_MODULE_LICENSE), ("THIRD_PARTY_NOTICES.md", LISP_MODULE_NOTICES)]) {
                    Ok(_) => {}
                    Err(e) => self.toast(i18n::fmt("Cannot set up the Lisp module: {}", &[&e]), Sev::Error),
                }
                self.settings.modules_welcome_done = true;
            }
            let _ = store.save_state(&state);
        }
        // tests and scripts: modules named here are installed before anything else (from the registry, no window)
        if let Ok(list) = std::env::var("LIGHT_PRELOAD_MODULES") {
            self.modules_preload(&list);
        }
        self.modules_reload();
    }

    fn modules_preload(&mut self, list: &str) {
        let Some(store) = self.mods.store.as_ref() else { return };
        let Ok((index, source)) = fetch_index(&self.registry_sources()) else { return };
        for id in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(e) = index.entry(id) {
                let _ = store.install(e, &source, &mut |_, _, _| {});
            }
        }
    }

    pub fn registry_sources(&self) -> Vec<Source> {
        default_sources(&self.settings.registry_url, bundled_registry().as_deref())
    }

    /// Read the installed modules again and rebuild the languages from the enabled ones.
    pub fn modules_reload(&mut self) {
        let Some(store) = self.mods.store.as_ref() else { return };
        let (installed, mut broken) = store.installed_with_errors();
        let mut langs = LanguageSet::new();
        let mut lisp = false;
        for m in installed.iter().filter(|m| m.enabled) {
            lisp |= m.manifest.builtin == "lisp";
            for l in &m.manifest.languages {
                if let Err(e) = langs.add(l.clone()) {
                    broken.push(format!("{}: {e}", m.manifest.id));
                }
            }
        }
        self.mods.installed = installed;
        self.mods.broken = broken;
        // the tools to look for in Linux depend on the modules that are on
        self.wsl.tools.clear();
        self.mods.langs = langs;
        self.mods.lisp = lisp;
        if !lisp {
            // the REPL goes with the Lisp module
            if let Some(s) = self.repl.session.take() {
                s.kill();
            }
            self.repl.ready = false;
            self.repl.starting = false;
            if self.bottom_tool == Some(BottomTool::Repl) {
                self.bottom_tool = None;
            }
        }
        self.apply_languages();
        self.recheck_all();
        self.refresh_needed_modules();
        self.refresh_make_targets();
    }

    // ------------------------------------------------------------------ languages

    pub fn lisp_enabled(&self) -> bool {
        self.mods.lisp
    }

    /// The language a file is read in: `None` is Lisp, which has an engine of its own.
    pub fn language_for(&self, path: Option<&Path>, first_line: Option<&str>) -> Option<Arc<Language>> {
        match path {
            Some(p) if is_lisp_file(p) => self.lisp_language(),
            Some(p) => Some(self.mods.langs.for_path(p, first_line)),
            None => Some(self.mods.langs.plain()),
        }
    }

    /// Lisp when the Lisp module is on, else plain text.
    fn lisp_language(&self) -> Option<Arc<Language>> {
        if self.mods.lisp { None } else { Some(self.mods.langs.plain()) }
    }

    /// Give a document the language its file calls for.
    pub fn assign_language(&self, d: &mut Doc) {
        let first = d.ed.text().lines().next().map(str::to_string);
        let lang = self.language_for(d.path.as_deref(), first.as_deref());
        d.set_language(lang);
    }

    /// The set of languages changed: every open document is read again in the right one.
    pub fn apply_languages(&mut self) {
        let mut docs = std::mem::take(&mut self.docs);
        for d in docs.iter_mut().filter(|d| d.diff.is_none()) {
            self.assign_language(d);
        }
        self.docs = docs;
    }

    /// The developer tools offered for Linux: what everyone wants, the groups of the modules that are on, and the tools
    /// the user added (one that is not written safely, or has the name of another group, is left out).
    pub fn tool_groups(&self) -> Vec<lisp_runtime::wsl::ToolGroup> {
        use lisp_runtime::wsl::{ToolGroup, custom_group, essential_groups};
        let mut groups = essential_groups();
        for m in self.mods.installed.iter().filter(|m| m.enabled) {
            for t in &m.manifest.tools {
                if !groups.iter().any(|g| g.name == t.name) {
                    groups.push(ToolGroup { name: t.name.clone(), programs: t.programs.clone(), packages: t.apt.clone(), custom: false });
                }
            }
        }
        for c in &self.settings.linux_tools.custom {
            if let Some(g) = custom_group(&c.name, &c.packages, &c.programs)
                && !groups.iter().any(|x| x.name == g.name)
            {
                groups.push(g);
            }
        }
        groups
    }

    /// What installing the developer tools puts into Linux: the groups cut down to the packages that are chosen on the
    /// Linux page. A group with a program of `force` (what a project cannot be built without) comes whole.
    pub fn chosen_tool_groups(&self, force: &[String]) -> Vec<lisp_runtime::wsl::ToolGroup> {
        let tools = &self.settings.linux_tools;
        self.tool_groups().into_iter().map(|g| if g.programs.iter().any(|p| force.contains(p)) { g } else { g.with_packages(|p| tools.is_chosen(p)) }).filter(|g| !g.packages.is_empty()).collect()
    }

    /// The id of the language of the active document (for run commands).
    pub fn active_language_id(&self) -> Option<String> {
        self.doc().and_then(|d| d.ed.language()).map(|l| l.id().to_string())
    }

    // ------------------------------------------------------------------ the registry

    /// Ask the registry what it offers (in the background); its covers are fetched after.
    pub fn modules_fetch_index(&mut self) {
        if self.mods.loading_index {
            return;
        }
        self.mods.loading_index = true;
        self.mods.loading_covers = true;
        self.mods.index_error = None;
        let sources = self.registry_sources();
        let tx = self.mods.tx.clone();
        let cache = self.mods.cache_dir();
        std::thread::spawn(move || {
            let result = fetch_index(&sources);
            if let Ok((index, source)) = &result {
                let _ = tx.send(Event::Index(Ok((index.clone(), source.clone()))));
                fetch_covers(index, source, cache.as_deref(), &tx);
                let _ = tx.send(Event::CoversDone);
            } else if let Err(e) = result {
                let _ = tx.send(Event::Index(Err(e)));
                let _ = tx.send(Event::CoversDone);
            }
        });
    }

    // ------------------------------------------------------------------ install, remove, switch

    pub fn module_install(&mut self, id: &str) {
        let (Some(store), Some(index), Some(source)) = (self.mods.store.as_ref(), self.mods.index.as_ref(), self.mods.index_source.clone()) else {
            self.toast(tr("The list of modules has not been loaded yet"), Sev::Weak);
            return;
        };
        let Some(entry) = index.entry(id).cloned() else {
            self.toast(i18n::fmt("The registry has no module {}", &[&id]), Sev::Error);
            return;
        };
        if self.mods.busy.contains_key(id) {
            return;
        }
        self.mods.busy.insert(id.to_string(), Busy { removing: false, done: 0, total: entry.files.len(), file: String::new() });
        let (dir, tx, id) = (store.dir().to_path_buf(), self.mods.tx.clone(), id.to_string());
        std::thread::spawn(move || {
            let store = ModuleStore::at(dir);
            let progress_tx = tx.clone();
            let pid = id.clone();
            let result = store.install(&entry, &source, &mut |done, total, file| {
                let _ = progress_tx.send(Event::Progress { id: pid.clone(), done, total, file: file.to_string() });
            });
            let _ = tx.send(Event::Finished { id, removed: false, result: Box::new(result) });
        });
    }

    /// Make `id` usable: switch it on if it is installed, else download it.
    pub fn module_use(&mut self, id: &str) {
        if self.mods.installed.iter().any(|m| m.manifest.id == id) {
            self.module_set_enabled(id, true);
        } else {
            self.module_install(id);
        }
    }

    pub fn module_remove(&mut self, id: &str) {
        let Some(store) = self.mods.store.as_ref() else { return };
        if self.mods.busy.contains_key(id) {
            return;
        }
        self.mods.busy.insert(id.to_string(), Busy { removing: true, done: 0, total: 1, file: String::new() });
        let (dir, tx, id) = (store.dir().to_path_buf(), self.mods.tx.clone(), id.to_string());
        std::thread::spawn(move || {
            let result = ModuleStore::at(dir).remove(&id).map(|_| Manifest::default());
            let _ = tx.send(Event::Finished { id, removed: true, result: Box::new(result) });
        });
    }

    pub fn module_set_enabled(&mut self, id: &str, on: bool) {
        let Some(store) = self.mods.store.as_ref() else { return };
        match store.set_enabled(id, on) {
            Ok(()) => self.modules_reload(),
            Err(e) => self.toast(i18n::fmt("Cannot switch the module: {}", &[&e]), Sev::Error),
        }
    }

    /// Collect what the background threads have done.
    pub fn poll_modules(&mut self, ctx: &egui::Context) {
        let mut reload = false;
        while let Ok(ev) = self.mods.rx.try_recv() {
            match ev {
                Event::Index(Ok((index, source))) => {
                    self.mods.covers.clear();
                    self.mods.index = Some(index);
                    self.mods.index_source = Some(source);
                    self.mods.loading_index = false;
                    self.refresh_needed_modules();
                }
                Event::Index(Err(e)) => {
                    self.mods.index_error = Some(e);
                    self.mods.loading_index = false;
                }
                // a picture that arrived is loaded when the tab next draws it
                Event::Cover(id) => {
                    self.mods.covers.remove(&id);
                    ctx.request_repaint();
                }
                Event::CoversDone => {
                    self.mods.loading_covers = false;
                    ctx.request_repaint();
                }
                Event::Progress { id, done, total, file } => {
                    if let Some(b) = self.mods.busy.get_mut(&id) {
                        b.done = done;
                        b.total = total;
                        b.file = file;
                    }
                }
                Event::Finished { id, removed, result } => {
                    self.mods.busy.remove(&id);
                    self.mods.covers.remove(&id);
                    let name = self.mods.index.as_ref().and_then(|i| i.entry(&id)).map_or(id.clone(), |e| e.name.clone());
                    match *result {
                        Ok(_) if removed => self.toast(i18n::fmt("{} is removed", &[&name]), Sev::Weak),
                        Ok(m) => self.toast(i18n::fmt("{0} {1} is installed", &[&m.name, &m.version]), Sev::Weak),
                        Err(e) => self.toast(i18n::fmt("Cannot install {0}: {1}", &[&name, &e]), Sev::Error),
                    }
                    reload = true;
                }
            }
        }
        if reload {
            self.modules_reload();
        }
        if !self.mods.busy.is_empty() || self.mods.loading_index || self.mods.loading_covers {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// The cover texture of a module, loaded from its file once (`None` while there is none).
    pub fn cover_texture(&mut self, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
        if let Some(t) = self.mods.covers.get(id) {
            return Some(t.clone());
        }
        let file = self.mods.cover_file(id)?;
        let bytes = std::fs::read(file).ok()?;
        let (w, h, rgba) = crate::brand::decode_png(&bytes)?;
        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        let t = ctx.load_texture(format!("cover-{id}"), img, TextureOptions::LINEAR);
        self.mods.covers.insert(id.to_string(), t.clone());
        Some(t)
    }

    // ------------------------------------------------------------------ what a project needs

    /// The modules the open project asks for or looks like it needs and that are not on yet.
    pub fn refresh_needed_modules(&mut self) {
        self.needed_modules.clear();
        let Some(p) = self.project.as_ref() else { return };
        let (exts, names) = ide_core::modules::detect::scan_files(p.files.iter().map(PathBuf::as_path));
        let mut wanted = self.project_cfg.modules.clone();
        for id in ide_core::modules::detect::needed_in(&exts, &names, &self.mods.detectors()) {
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        // the modules that are on and have files here: the tools they bring are what the project needs in Linux
        let in_use = ide_core::modules::detect::needed_in(&exts, &names, &self.mods.detectors());
        self.project_modules = in_use.into_iter().filter(|id| self.mods.installed.iter().any(|m| &m.manifest.id == id && m.enabled && !m.manifest.tools.is_empty())).collect();
        self.needed_modules = wanted.into_iter().filter(|id| !self.mods.is_enabled(id)).collect();
        self.update_env_banner();
    }
}

/// Fetch the cover pictures the cache does not have yet.
fn fetch_covers(index: &Index, source: &Source, cache: Option<&Path>, tx: &Sender<Event>) {
    let Some(cache) = cache else { return };
    let _ = std::fs::create_dir_all(cache);
    for e in &index.modules {
        let Some(f) = cover_entry(e) else { continue };
        let file = cache.join(cache_name(&e.id, f));
        if file.is_file() {
            continue;
        }
        if let Ok(bytes) = e.source.as_ref().unwrap_or(source).fetch(&format!("{}/{}", e.path, f.name))
            && bytes.len() as u64 == f.size
            && ide_core::modules::sha256::sha256_hex(&bytes).eq_ignore_ascii_case(&f.sha256)
            && std::fs::write(&file, bytes).is_ok()
        {
            let _ = tx.send(Event::Cover(e.id.clone()));
        }
    }
}

fn cover_entry(e: &IndexEntry) -> Option<&FileEntry> {
    e.files.iter().find(|f| f.name == e.cover)
}

#[cfg(test)]
mod tests {
    use ide_core::settings::{CustomTool, Settings};

    use crate::app::App;
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("light-module-art-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn refreshed_artwork_does_not_require_updating_installed_module_code() {
        let fixture = Fixture::new();
        let store = ModuleStore::at(fixture.0.join("modules"));
        let mut manifest = Manifest::parse(std::str::from_utf8(include_bytes!("../../../registry/modules/python/module.json")).unwrap()).unwrap();
        manifest.version = "1.0.0".into();
        let json = serde_json::to_vec(&manifest).unwrap();
        store.install_bytes("python", &[("module.json", &json), ("cover.png", b"old cover with title")]).unwrap();
        let mut state = ModulesState { installed: store.installed(), store: Some(store), ..Default::default() };
        let installed_cover = state.installed[0].cover_path().unwrap();
        // Before a network response, the updated cover shipped with Light works offline.
        assert_ne!(state.cover_file("python").unwrap(), installed_cover);
        assert_eq!(std::fs::read(&installed_cover).unwrap(), b"old cover with title");
        let index = Index::parse(include_str!("../../../registry/index.json")).unwrap();
        let entry = index.entry("python").unwrap();
        let current_cover = state.cache_dir().unwrap().join(cache_name("python", cover_entry(entry).unwrap()));
        std::fs::create_dir_all(current_cover.parent().unwrap()).unwrap();
        std::fs::write(&current_cover, b"new catalog artwork").unwrap();
        state.index = Some(index);
        assert_eq!(state.cover_file("python").unwrap(), current_cover);
        assert_eq!(state.installed[0].manifest.version, "1.0.0");
        // A third-party module without bundled/catalog art keeps its installed cover.
        manifest.id = "custom-art".into();
        let json = serde_json::to_vec(&manifest).unwrap();
        state.store.as_ref().unwrap().install_bytes("custom-art", &[("module.json", &json), ("cover.png", b"custom art")]).unwrap();
        state.installed = state.store.as_ref().unwrap().installed();
        assert_eq!(std::fs::read(state.cover_file("custom-art").unwrap()).unwrap(), b"custom art");
    }

    #[test]
    fn downloaded_cover_invalidates_only_its_loaded_texture() {
        let ctx = egui::Context::default();
        let mut app = App::new(Settings::default());
        let texture = ctx.load_texture("cover-fixture", egui::ColorImage::filled([1, 1], egui::Color32::WHITE), TextureOptions::LINEAR);
        app.mods.covers.insert("python".into(), texture.clone());
        app.mods.covers.insert("lisp".into(), texture);
        app.mods.tx.send(Event::Cover("python".into())).unwrap();
        app.poll_modules(&ctx);
        assert!(!app.mods.covers.contains_key("python"));
        assert!(app.mods.covers.contains_key("lisp"));
    }

    #[test]
    fn lisp_settings_follow_module_install_enable_disable_and_remove() {
        let fixture = Fixture::new();
        let store = ModuleStore::at(fixture.0.join("modules"));
        let mut app = App::new(Settings::default());
        app.mods.store = Some(store);
        let visible = |app: &App| crate::ui_settings::page_visible(crate::ui_settings::LISP_PAGE, app.lisp_enabled());
        app.modules_reload();
        assert!(!visible(&app));
        app.mods.store.as_ref().unwrap().install_bytes("lisp", &[("module.json", LISP_MODULE_JSON), ("cover.png", LISP_MODULE_COVER)]).unwrap();
        app.modules_reload();
        assert!(visible(&app));
        app.mods.store.as_ref().unwrap().set_enabled("lisp", false).unwrap();
        app.modules_reload();
        assert!(!visible(&app));
        app.mods.store.as_ref().unwrap().set_enabled("lisp", true).unwrap();
        app.modules_reload();
        assert!(visible(&app));
        app.mods.store.as_ref().unwrap().remove("lisp").unwrap();
        app.modules_reload();
        assert!(!visible(&app));
        assert!(crate::ui_settings::page_visible(crate::ui_settings::WSL_PAGE, app.lisp_enabled()));
    }

    #[test]
    fn documents_without_a_path_use_plain_text_even_with_lisp_enabled() {
        let mut app = App::new(Settings::default());
        app.mods.lisp = true;
        let mut doc = crate::doc::Doc::untitled(1);
        app.assign_language(&mut doc);
        assert!(!doc.ed.is_lisp());
        assert!(app.language_for(Some(std::path::Path::new("main.lisp")), None).is_none());
        assert_eq!(app.language_for(None, None).unwrap().id(), "plain");
    }

    fn packages(app: &App, force: &[&str]) -> Vec<String> {
        let force: Vec<String> = force.iter().map(|s| s.to_string()).collect();
        app.chosen_tool_groups(&force).iter().flat_map(|g| g.package_list().into_iter().map(String::from).collect::<Vec<_>>()).collect()
    }

    #[test]
    fn the_groups_to_install_follow_the_choice() {
        let mut app = App::new(Settings::default());
        let all = packages(&app, &[]);
        assert!(all.contains(&"git".to_string()) && all.contains(&"vim".to_string()) && all.contains(&"doxygen".to_string()), "everything by default: {all:?}");
        app.settings.linux_tools.choose("vim", false);
        for p in ["man-db", "manpages-dev", "doxygen"] {
            app.settings.linux_tools.choose(p, false);
        }
        let some = packages(&app, &[]);
        assert!(!some.contains(&"vim".to_string()) && !some.contains(&"doxygen".to_string()) && some.contains(&"nano".to_string()));
        // a group with nothing chosen is not installed at all
        assert!(!app.chosen_tool_groups(&[]).iter().any(|g| g.name == "Documentation"));
        // what is looked for is still every program: the page shows what is missing even if it is left out
        let looked_for = lisp_runtime::wsl::all_programs(&app.tool_groups());
        assert!(looked_for.contains(&"vim".to_string()) && looked_for.contains(&"doxygen".to_string()));
        // a program the project cannot do without brings its group whole, chosen or not
        let forced = packages(&app, &["doxygen"]);
        assert!(forced.contains(&"doxygen".to_string()) && forced.contains(&"man-db".to_string()) && !forced.contains(&"vim".to_string()), "{forced:?}");
        // nothing chosen: nothing to install
        for p in &all {
            app.settings.linux_tools.choose(p, false);
        }
        assert!(app.chosen_tool_groups(&[]).is_empty());
    }

    #[test]
    fn tools_of_ones_own_join_the_groups_unless_they_are_unsafe_or_clash() {
        let mut app = App::new(Settings::default());
        let tool = |n: &str, p: &str, r: &str| CustomTool { name: n.into(), packages: p.into(), programs: r.into() };
        app.settings.linux_tools.custom = vec![tool("Libraries", "libssl-dev zlib1g-dev", ""), tool("Network", "tcpdump nmap", "tcpdump"), tool("Evil", "x; reboot", ""), tool("Documentation", "nmap", ""), tool("Libraries", "other", "")];
        let groups = app.tool_groups();
        let names: Vec<&str> = groups.iter().filter(|g| g.custom).map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Libraries", "Network"], "an unsafe one and a clash with another group are left out");
        let libs = groups.iter().find(|g| g.name == "Libraries").unwrap();
        assert_eq!(libs.packages, "libssl-dev zlib1g-dev");
        assert_eq!(libs.programs, ["dpkg:libssl-dev", "dpkg:zlib1g-dev"]);
        assert!(!groups.iter().find(|g| g.name == "Documentation").unwrap().custom, "the group of the same name stays the built-in one");
        // they are chosen like the others
        app.settings.linux_tools.choose("nmap", false);
        assert!(!packages(&app, &[]).contains(&"nmap".to_string()) && packages(&app, &[]).contains(&"tcpdump".to_string()));
    }
}

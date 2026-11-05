//! Background checks/downloads; activation waits until the installation/module store is unused.
use std::{fs::{self, File}, path::{Path, PathBuf}, process::{Command, Stdio}, sync::mpsc};
use ide_core::{modules::{Index, IndexEntry, ModuleStore, is_newer}, settings::Settings, updates as engine};
use serde::{Deserialize, Serialize};
use crate::{app::App, editor_view::Sev, i18n::tr};

const KEY: &str = include_str!("../../../assets/updates/public-key.hex");
const INTERVAL: u64 = 6 * 3600;

#[derive(Clone, Serialize, Deserialize)]
struct PendingIde { envelope: String, destination: PathBuf, base_version: String }
#[derive(Clone, Serialize, Deserialize)]
struct PendingModules { envelope: String, old: Vec<IndexEntry> }
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Stamp { checked: u64, status: String, available: Option<String>, skipped: Vec<String>, installer: Option<PathBuf> }
#[derive(Clone, Serialize, Deserialize)]
struct LastInstall { version: String, previous_version: String, backup: PathBuf, installation: PathBuf }
#[derive(Serialize, Deserialize)]
struct ModuleRollback { id: String, version: String }
#[derive(Default)]
struct Outcome { status: String, available: Option<String>, ready: bool, installer: Option<PathBuf> }
#[derive(Default)]
pub struct UpdateState {
    pub status: String,
    pub available: Option<String>,
    pub ready: bool,
    pub working: bool,
    pub can_rollback: bool,
    installer: Option<PathBuf>,
    apply_requested: bool,
    rollback_requested: bool,
    root: Option<PathBuf>,
    installation: Option<PathBuf>,
    installation_lock: Option<File>,
    module_lock: Option<File>,
    next_check: u64,
    receiver: Option<mpsc::Receiver<Outcome>>,
}

fn root() -> Option<PathBuf> { Settings::config_dir().map(|p| p.join("updates")) }
fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, String> { serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string()) }
fn install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let parent = exe.parent()?;
    let root = if cfg!(target_os = "macos") && parent.file_name().is_some_and(|n| n == "MacOS") { parent.parent()?.parent()? } else { parent };
    let inventory = engine::read_inventory(root).ok()?;
    engine::version(&inventory.version).ok()?;
    if root.join(".light-package-manager").exists() { return None }
    Some(root.to_path_buf())
}
fn install_lock(cache: &Path, install: &Path) -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| cache.to_path_buf()).join("Light/update-locks").join(format!("install-{}.lock", ide_core::modules::sha256::sha256_hex(install.to_string_lossy().as_bytes())))
}
fn pending_ide(cache: &Path, installation: &Path) -> PathBuf {
    cache.join(format!("pending-ide-{}.json", ide_core::modules::sha256::sha256_hex(installation.to_string_lossy().as_bytes())))
}
fn install_record(cache: &Path, installation: &Path, label: &str) -> PathBuf {
    cache.join(format!("{label}-{}.json", ide_core::modules::sha256::sha256_hex(installation.to_string_lossy().as_bytes())))
}
fn update_platform(installation: &Path) -> String {
    let platform = engine::platform();
    if cfg!(target_os = "macos") && engine::read_inventory(installation).is_ok_and(|i| !i.entrypoint.starts_with("Contents/MacOS/")) { format!("{platform}-portable") } else { platform }
}
fn modules_lock(cache: &Path) -> PathBuf {
    let store = ModuleStore::default_dir().unwrap_or_else(|| cache.join("modules"));
    let _ = fs::create_dir_all(&store);
    let store = store.canonicalize().unwrap_or(store);
    dirs::data_local_dir().unwrap_or_else(|| cache.to_path_buf()).join("Light/update-locks").join(format!("modules-{}.lock", ide_core::modules::sha256::sha256_hex(store.to_string_lossy().as_bytes())))
}

/// A full IDE must not read a bundle while the helper atomically replaces it.
/// Quick View returns before this guard and never reads installed module state.
pub fn protect_startup() -> Result<Option<File>, String> {
    let Some(installation) = install_root() else { return Ok(None) };
    let cache = root().ok_or("update cache is unavailable")?;
    let started = std::time::Instant::now();
    loop {
        if let Ok(guard) = engine::lock(&install_lock(&cache, &installation), false) {
            let inventory = engine::read_inventory(&installation)?;
            if inventory.version != env!("CARGO_PKG_VERSION") {
                if std::env::var("LIGHT_UPDATE_RESTARTED").is_ok_and(|v| v == env!("CARGO_PKG_VERSION")) { return Err("bundle version does not match its executable".into()); }
                Command::new(std::env::current_exe().map_err(|e| e.to_string())?).args(std::env::args_os().skip(1)).env("LIGHT_UPDATE_RESTARTED", env!("CARGO_PKG_VERSION")).spawn().map_err(|e| e.to_string())?;
                std::process::exit(0);
            }
            return Ok(Some(guard));
        }
        if started.elapsed().as_secs() >= 60 { return Err("Light is still applying an update. The previous installation is retained; try again shortly.".into()); }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
fn catalog(cache: &Path, envelope: &str) -> Result<(engine::Manifest, Index), String> {
    let manifest = engine::verify(envelope.as_bytes(), KEY, "modules", engine::now())?;
    let artifact = manifest.targets.get("catalog").ok_or("signed catalog target is missing")?;
    let path = cache.join("catalog").join(&manifest.version).join(&artifact.name);
    engine::check_file(&path, artifact)?;
    let index = Index::parse(&fs::read_to_string(&path).map_err(|e| e.to_string())?)?;
    Ok((manifest, index))
}
fn module_archive(cache: &Path, entry: &IndexEntry) -> Result<PathBuf, String> {
    let archive = entry.archive.as_ref().ok_or("module has no verified archive")?;
    if !archive.url.starts_with("https://github.com/sherhn/Light-Modules/releases/download/") { return Err("automatic module updates require official signed packages".into()); }
    Ok(cache.join("modules").join(&archive.sha256).join(format!("{}-{}.zip", entry.id, entry.version)))
}
fn apply_modules(cache: &Path, pins: &std::collections::BTreeSet<String>) -> Result<usize, String> {
    let path = cache.join("pending-modules.json");
    if !path.is_file() { return Ok(0) }
    let _guard = engine::lock(&modules_lock(cache), true)?;
    let pending: PendingModules = read_json(&path)?;
    let (_, index) = catalog(cache, &pending.envelope)?;
    let store = ModuleStore::at(ModuleStore::default_dir().ok_or("module store is unavailable")?);
    let mut count = 0;
    for old in pending.old {
        if pins.contains(&old.id) { continue }
        let Some(entry) = index.entry(&old.id).filter(|e| e.compatible() && is_newer(&e.version, &old.version)) else { continue };
        // Recheck at activation, not merely when downloading: the user may have edited a local module meanwhile.
        if !store.matches_entry(&old) { continue }
        store.mark_managed(&old)?;
        let archive = entry.archive.as_ref().unwrap();
        let file = module_archive(cache, entry)?;
        engine::check_file(&file, &engine::Artifact { name: file.file_name().unwrap().to_string_lossy().into_owned(), url: archive.url.clone(), size: archive.size, sha256: archive.sha256.clone() })?;
        store.install_archive(entry, &fs::read(file).map_err(|e| e.to_string())?, &mut |_, _, _| {})?;
        count += 1;
    }
    fs::remove_file(path).map_err(|e| e.to_string())?;
    Ok(count)
}

fn apply_module_rollbacks(cache: &Path) -> Result<(), String> {
    let path = cache.join("rollback-modules.json");
    if !path.is_file() { return Ok(()) }
    let _guard = engine::lock(&modules_lock(cache), true)?;
    let store = ModuleStore::at(ModuleStore::default_dir().ok_or("module store is unavailable")?);
    for request in read_json::<Vec<ModuleRollback>>(&path)? { store.rollback_to(&request.id, &request.version)?; }
    fs::remove_file(path).map_err(|e| e.to_string())
}

fn check(cache: &Path, installation: Option<&Path>, download_ide: bool, download_modules: bool, official_modules: bool, force: bool, pins: &std::collections::BTreeSet<String>) -> Outcome {
    let mut result = Outcome::default(); let mut notes = Vec::new();
    let ide = (|| -> Result<(), String> {
        let envelope = engine::fetch_metadata(engine::IDE_FEED)?;
        let metadata = engine::verify(&envelope, KEY, "ide", engine::now())?;
        if engine::version(&metadata.version)? <= engine::version(env!("CARGO_PKG_VERSION"))? { return Ok(()) }
        result.available = Some(metadata.version.clone());
        if !force && read_json::<Stamp>(&cache.join("state.json")).unwrap_or_default().skipped.contains(&metadata.version) { return Ok(()) }
        if !download_ide { return Ok(()) }
        let Some(installation) = installation else {
            // Never replace dpkg-owned files. Download the signed native package for the OS installer.
            if cfg!(target_os = "linux") && std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.join(".light-package-manager"))).is_some_and(|p| p.is_file()) {
                let artifact = metadata.targets.get(&format!("{}-installer", engine::platform())).ok_or("this platform has no native update package")?;
                result.installer = Some(engine::download(artifact, &cache.join("installers").join(&metadata.version))?);
            }
            return Ok(())
        };
        engine::validate_tree(installation, env!("CARGO_PKG_VERSION"))?;
        let artifact = metadata.targets.get(&update_platform(installation)).ok_or("this platform has no automatic update")?;
        let archive = engine::download(artifact, &cache.join("ide").join(&metadata.version))?;
        engine::unpack(&archive, &cache.join("ide").join(&metadata.version).join("unpacked"), &metadata.version)?;
        let pending = PendingIde { envelope: String::from_utf8(envelope).map_err(|e| e.to_string())?, destination: installation.to_path_buf(), base_version: env!("CARGO_PKG_VERSION").into() };
        engine::atomic_json(&pending_ide(cache, installation), &pending)?;
        result.ready = true; Ok(())
    })();
    if let Err(error) = ide { notes.push(format!("IDE: {error}")); }
    if download_modules && official_modules {
        let modules = (|| -> Result<usize, String> {
            let envelope = engine::fetch_metadata(engine::MODULE_FEED)?;
            let manifest = engine::verify(&envelope, KEY, "modules", engine::now())?;
            let artifact = manifest.targets.get("catalog").ok_or("signed catalog target is missing")?;
            let path = engine::download(artifact, &cache.join("catalog").join(&manifest.version))?;
            let index = Index::parse(&fs::read_to_string(path).map_err(|e| e.to_string())?)?;
            let baseline = Index::parse(include_str!("../../../registry/index.json"))?;
            let store = ModuleStore::at(ModuleStore::default_dir().ok_or("module store is unavailable")?);
            let mut old = Vec::new();
            for installed in store.installed() {
                if pins.contains(&installed.manifest.id) { continue }
                let Some(entry) = index.entry(&installed.manifest.id).filter(|e| e.compatible() && is_newer(&e.version, &installed.manifest.version)) else { continue };
                let proof = store.managed_entry(&entry.id).or_else(|| baseline.entry(&entry.id).filter(|e| store.matches_entry(e)).cloned());
                let Some(proof) = proof else { continue }; // Local/customized modules are never overwritten automatically.
                let archive = entry.archive.as_ref().ok_or("module has no archive")?;
                let file = module_archive(cache, entry)?;
                engine::download(&engine::Artifact { name: file.file_name().unwrap().to_string_lossy().into_owned(), url: archive.url.clone(), size: archive.size, sha256: archive.sha256.clone() }, file.parent().unwrap())?;
                old.push(proof);
            }
            let count = old.len();
            if count > 0 { engine::atomic_json(&cache.join("pending-modules.json"), &PendingModules { envelope: String::from_utf8(envelope).map_err(|e| e.to_string())?, old })?; }
            Ok(count)
        })();
        match modules { Ok(n) if n > 0 => notes.push(format!("{n} module update(s) will be activated on the next start")), Ok(_) => (), Err(e) => notes.push(format!("Modules: {e}")) }
    }
    let headline = if result.ready { "IDE update downloaded; it will be applied after closing Light" } else if result.installer.is_some() { "Native update package downloaded; install it with your system package manager" } else if result.available.is_some() { "A newer IDE version is available" } else { "Update check completed" };
    result.status = std::iter::once(headline.to_string()).chain(notes).collect::<Vec<_>>().join("\n");
    let prior = read_json::<Stamp>(&cache.join("state.json")).unwrap_or_default();
    let _ = engine::atomic_json(&cache.join("state.json"), &Stamp { checked: engine::now(), status: result.status.clone(), available: result.available.clone(), installer: result.installer.clone(), ..prior });
    result
}

impl App {
    pub fn updates_init(&mut self) -> bool {
        let Some(cache) = root() else { return true };
        self.updater.installation = install_root();
        if let Some(install) = &self.updater.installation {
            self.updater.installation_lock = engine::lock(&install_lock(&cache, install), false).ok();
        }
        if let Err(e) = apply_module_rollbacks(&cache) { self.updater.status = e; }
        if self.settings.auto_update_modules && (self.settings.registry_url.is_empty() || self.settings.registry_url.trim_end_matches('/') == ide_core::modules::registry::DEFAULT_URL)
            && let Err(e) = apply_modules(&cache, &self.settings.module_update_pins) { self.updater.status = e; }
        let started = std::time::Instant::now();
        loop {
            if let Ok(guard) = engine::lock(&modules_lock(&cache), false) { self.updater.module_lock = Some(guard); break }
            if started.elapsed().as_secs() >= 60 { self.updater.status = tr("Module update is still running; reopen Light shortly").into(); break }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if let Ok(stamp) = read_json::<Stamp>(&cache.join("state.json")) {
            self.updater.next_check = stamp.checked.saturating_add(INTERVAL);
            self.updater.available = stamp.available;
            self.updater.installer = stamp.installer.filter(|p| p.is_file() && p.starts_with(cache.join("installers")));
            if self.updater.status.is_empty() { self.updater.status = stamp.status; }
        }
        self.updater.ready = self.updater.installation.as_ref().is_some_and(|install| pending_ide(&cache, install).is_file());
        self.updater.can_rollback = self.updater.installation.as_ref().is_some_and(|install| read_json::<LastInstall>(&install_record(&cache, install, "last-install")).is_ok_and(|r| r.installation == *install && r.version == env!("CARGO_PKG_VERSION") && r.backup.is_dir()));
        if let Some(install) = &self.updater.installation {
            let request = install_record(&cache, install, "rollback-ide");
            if let Ok(record) = read_json::<LastInstall>(&request) {
                if record.version == env!("CARGO_PKG_VERSION") { self.updater.rollback_requested = true; }
                else { let _ = fs::remove_file(request); }
            }
        }
        self.updater.root = Some(cache);
        self.updater.module_lock.is_some()
    }
    pub fn check_updates(&mut self, download_now: bool) {
        if self.updater.working { return }
        let Some(cache) = self.updater.root.clone() else { self.updater.status = tr("Updates are unavailable in this environment").into(); return };
        let install = self.updater.installation.clone();
        let download_ide = self.settings.auto_update_ide || download_now;
        let download_modules = self.settings.auto_update_modules;
        let official = self.settings.registry_url.is_empty() || self.settings.registry_url.trim_end_matches('/') == ide_core::modules::registry::DEFAULT_URL;
        let pins = self.settings.module_update_pins.clone();
        let (tx, rx) = mpsc::channel(); self.updater.receiver = Some(rx); self.updater.working = true;
        self.updater.status = tr("Checking for signed updates...").into();
        std::thread::spawn(move || {
            let result = match engine::lock(&cache.join("locks/download.lock"), true) {
                Ok(_guard) => check(&cache, install.as_deref(), download_ide, download_modules, official, download_now, &pins),
                Err(e) => Outcome { status: e, ..Default::default() },
            };
            let _ = tx.send(result);
        });
    }
    pub fn poll_updates(&mut self, ctx: &eframe::egui::Context) {
        // Quick View never constructs App. Scripted/test windows never use the live feeds.
        if self.script.is_some() || std::env::var_os("LIGHT_DISABLE_AUTO_UPDATES").is_some() { return }
        if let Some(rx) = &self.updater.receiver {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Outcome { status: tr("Update check stopped unexpectedly; you can retry").into(), ..Default::default() }),
            };
            if let Some(result) = result {
                self.updater.status = result.status; self.updater.available = result.available; self.updater.ready = result.ready; self.updater.installer = result.installer;
                self.updater.working = false; self.updater.receiver = None;
                self.updater.next_check = engine::now() + INTERVAL;
            }
        }
        if self.now > 10.0 && engine::now() >= self.updater.next_check && (self.settings.auto_update_ide || self.settings.auto_update_modules) { self.check_updates(false); }
        if self.updater.working { ctx.request_repaint_after(std::time::Duration::from_millis(250)); }
    }
    pub fn start_update_after_exit(&mut self) {
        if self.script.is_some() || (!self.updater.rollback_requested && ((!self.settings.auto_update_ide && !self.updater.apply_requested) || !self.updater.ready)) { return }
        let Some(cache) = &self.updater.root else { return };
        let Some(installation) = &self.updater.installation else { return };
        let (pending, mode) = if self.updater.rollback_requested { (install_record(cache, installation, "rollback-ide"), "--light-rollback-update") } else { (pending_ide(cache, installation), "--light-apply-update") };
        let result = (|| -> Result<(), String> {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            let helpers = cache.join("helpers").join(std::process::id().to_string());
            let helper = if cfg!(target_os = "macos") && installation.join("Contents/Frameworks").is_dir() {
                // Relocated dylibs use @loader_path/../Frameworks. Keep that layout for the detached helper.
                for (relative, source) in engine::tree_files(&installation.join("Contents/Frameworks"))? {
                    let destination = helpers.join("Frameworks").join(relative);
                    fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
                    fs::copy(source, destination).map_err(|e| e.to_string())?;
                }
                helpers.join("MacOS/light")
            } else { helpers.join(if cfg!(windows) { "light.exe" } else { "light" }) };
            fs::create_dir_all(helper.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(executable, &helper).map_err(|e| e.to_string())?;
            let mut command = Command::new(helper); command.arg(mode).arg(pending).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x0800_0000); }
            command.spawn().map_err(|e| e.to_string())?; Ok(())
        })();
        if let Err(e) = result { self.toast(e, Sev::Warning); }
    }

    fn queue_ide_rollback(&mut self) -> Result<(), String> {
        let cache = self.updater.root.as_ref().ok_or("update cache is unavailable")?;
        let install = self.updater.installation.as_ref().ok_or("not an update-managed installation")?;
        let record: LastInstall = read_json(&install_record(cache, install, "last-install"))?;
        if record.installation != *install || record.version != env!("CARGO_PKG_VERSION") { return Err("rollback does not match this installation".into()); }
        let previous = engine::read_inventory(&record.backup)?; engine::validate_tree(&record.backup, &previous.version)?;
        engine::atomic_json(&install_record(cache, install, "rollback-ide"), &record)?;
        let mut stamp = read_json::<Stamp>(&cache.join("state.json")).unwrap_or_default();
        if !stamp.skipped.contains(&record.version) { stamp.skipped.push(record.version); }
        stamp.status = tr("Rollback queued; close Light when ready. This update will not be installed again automatically.").into();
        engine::atomic_json(&cache.join("state.json"), &stamp)?;
        self.updater.status = stamp.status; self.updater.rollback_requested = true;
        Ok(())
    }

    pub fn queue_module_rollback(&mut self, id: &str) {
        let result = (|| -> Result<(), String> {
            let cache = self.updater.root.as_ref().ok_or("update cache is unavailable")?;
            let store = ModuleStore::at(ModuleStore::default_dir().ok_or("module store is unavailable")?);
            let version = store.previous_version(id).ok_or("a verified previous module version is unavailable")?;
            let path = cache.join("rollback-modules.json");
            let mut ids = read_json::<Vec<ModuleRollback>>(&path).unwrap_or_default();
            ids.retain(|r| r.id != id); ids.push(ModuleRollback { id: id.to_string(), version });
            engine::atomic_json(&path, &ids)?;
            self.settings.module_update_pins.insert(id.into()); self.save_settings();
            Ok(())
        })();
        match result { Ok(()) => self.toast(tr("Previous module version will be restored on the next start; automatic updates for it are paused"), Sev::Weak), Err(e) => self.toast(e, Sev::Error) }
    }

    pub fn draw_updates_settings(&mut self, ui: &mut eframe::egui::Ui) {
        ui.heading(tr("Updates"));
        ui.add_space(10.0);
        ui.checkbox(&mut self.settings.auto_update_ide, tr("Download IDE updates in the background"));
        ui.checkbox(&mut self.settings.auto_update_modules, tr("Download official module updates in the background"));
        ui.label(tr("Signed packages are checked every six hours. IDE updates apply after closing Light; modules activate on the next start. Previous versions are retained."));
        ui.label(tr("Local modifications and custom registries are never overwritten automatically."));
        if self.updater.installation.is_none() {
            ui.label(tr("This copy is managed by a package manager or is a development build. Install a release bundle to enable automatic IDE replacement."));
        }
        ui.add_space(12.0);
        if ui.add_enabled(!self.updater.working, eframe::egui::Button::new(tr("Check for updates"))).clicked() { self.check_updates(false); }
        if self.updater.available.is_some() && !self.updater.ready && self.updater.installation.is_some()
            && ui.add_enabled(!self.updater.working, eframe::egui::Button::new(tr("Download update now"))).clicked() { self.check_updates(true); }
        if self.updater.working { ui.spinner(); }
        if let Some(installer) = &self.updater.installer && ui.button(tr("Show downloaded system package")).clicked() { crate::input::reveal_in_file_manager(installer); }
        if self.updater.ready && ui.button(tr("Apply update after closing Light")).clicked() { self.updater.apply_requested = true; self.updater.status = tr("Update queued; close Light when ready").into(); }
        if self.updater.can_rollback && ui.button(tr("Restore previous IDE version after closing")).clicked() && let Err(e) = self.queue_ide_rollback() { self.toast(e, Sev::Error); }
        if !self.updater.status.is_empty() {
            let status = self.updater.status.lines().map(|line| crate::i18n::tr_dyn(line).to_string()).collect::<Vec<_>>().join("\n");
            ui.add(eframe::egui::Label::new(status).wrap());
            if ui.button(tr("Copy update status")).clicked() { ui.ctx().copy_text(self.updater.status.clone()); }
        }
    }
}

fn apply_ide(path: &Path) -> Result<(), String> {
    let cache = root().ok_or("updates cache is unavailable")?;
    let _cache_guard = engine::lock(&cache.join("locks/download.lock"), true)?;
    if path.parent() != Some(cache.as_path()) { return Err("invalid pending update location".into()); }
    let pending: PendingIde = read_json(path)?;
    if path != pending_ide(&cache, &pending.destination) { return Err("pending update destination mismatch".into()); }
    let mut guard = None;
    for _ in 0..60 {
        if let Ok(lock) = engine::lock(&install_lock(&cache, &pending.destination), true) { guard = Some(lock); break }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let _guard = guard.ok_or("another Light instance is active; installation was not modified")?;
    let metadata = engine::verify(pending.envelope.as_bytes(), KEY, "ide", engine::now())?;
    let current = engine::read_inventory(&pending.destination)?;
    if engine::version(&current.version)? >= engine::version(&metadata.version)? {
        fs::remove_file(path).map_err(|e| e.to_string())?; return Ok(());
    }
    let artifact = metadata.targets.get(&update_platform(&pending.destination)).ok_or("update platform mismatch")?;
    let archive = cache.join("ide").join(&metadata.version).join(&artifact.name);
    engine::check_file(&archive, artifact)?;
    engine::validate_tree(&pending.destination, &pending.base_version)?;
    let unpacked = cache.join("ide").join(&metadata.version).join("unpacked");
    engine::unpack(&archive, &unpacked, &metadata.version)?;
    let predicted = pending.destination.parent().ok_or("installation has no parent")?.join(format!(".{}.previous-{}", pending.destination.file_name().unwrap().to_string_lossy(), pending.base_version));
    let record_path = install_record(&cache, &pending.destination, "last-install");
    engine::atomic_json(&record_path, &LastInstall { version: metadata.version.clone(), previous_version: pending.base_version.clone(), backup: predicted, installation: pending.destination.clone() })?;
    let backup = engine::replace(&pending.destination, &unpacked, &metadata.version, |exe| health(exe, &metadata.version))?;
    engine::atomic_json(&record_path, &LastInstall { version: metadata.version, previous_version: pending.base_version, backup, installation: pending.destination })?;
    fs::remove_file(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn health(exe: &Path, version: &str) -> Result<(), String> {
        let mut command = Command::new(exe); command.arg("--light-update-self-check").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(cache) = root() {
            let isolated = cache.join("health").join(version);
            command.env("LIGHT_CONFIG_DIR", &isolated).env("LIGHT_MODULES_DIR", isolated.join("modules")).env("LIGHT_DISABLE_AUTO_UPDATES", "1");
        }
        #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x0800_0000); }
        let mut child = command.spawn().map_err(|e| e.to_string())?; let start = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                let output = child.wait_with_output().map_err(|e| e.to_string())?;
                if status.success() && String::from_utf8_lossy(&output.stdout).lines().any(|line| line.trim() == format!("LIGHT_UPDATE_OK {version}")) { return Ok(()) }
                return Err("updated executable failed its startup check".into());
            }
            if start.elapsed().as_secs() >= 30 { let _ = child.kill(); let _ = child.wait(); return Err("updated executable timed out during startup check".into()); }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
}

fn rollback_ide(path: &Path) -> Result<(), String> {
    let cache = root().ok_or("updates cache is unavailable")?;
    if path.parent() != Some(cache.as_path()) { return Err("invalid rollback request location".into()); }
    let record: LastInstall = read_json(path)?;
    if path != install_record(&cache, &record.installation, "rollback-ide") { return Err("rollback destination mismatch".into()); }
    let mut guard = None;
    for _ in 0..60 {
        if let Ok(lock) = engine::lock(&install_lock(&cache, &record.installation), true) { guard = Some(lock); break }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let _guard = guard.ok_or("another Light instance is still running")?;
    let current = engine::read_inventory(&record.installation)?;
    if current.version == record.previous_version { let _ = fs::remove_file(path); return Ok(()) }
    if current.version != record.version { return Err("IDE rollback request is stale".into()); }
    let previous = engine::read_inventory(&record.backup)?;
    if previous.version != record.previous_version { return Err("IDE rollback backup version mismatch".into()); }
    let saved = engine::restore(&record.installation, &record.backup, |exe| health(exe, &previous.version))?;
    engine::atomic_json(&install_record(&cache, &record.installation, "last-install"), &LastInstall { version: previous.version, previous_version: record.version, backup: saved, installation: record.installation.clone() })?;
    let _ = fs::remove_file(pending_ide(&cache, &record.installation));
    fs::remove_file(path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn early_mode() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--light-update-self-check") {
        if crate::brand::window_icon(false).is_none() { return Some(1) }
        println!("LIGHT_UPDATE_OK {}", env!("CARGO_PKG_VERSION")); return Some(0)
    }
    if args.get(1).is_some_and(|a| a == "--light-apply-update" || a == "--light-rollback-update") {
        let rollback = args[1] == "--light-rollback-update";
        let result = args.get(2).ok_or("pending update file missing".to_string()).and_then(|p| if rollback { rollback_ide(Path::new(p)) } else { apply_ide(Path::new(p)) });
        if let Err(error) = result {
            if let Some(cache) = root() { let _ = engine::atomic_json(&cache.join("state.json"), &Stamp { checked: engine::now(), status: error, available: None, ..read_json::<Stamp>(&cache.join("state.json")).unwrap_or_default() }); }
            return Some(1)
        }
        return Some(0)
    }
    None
}

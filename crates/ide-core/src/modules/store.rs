//! The modules that are installed: a folder per module in the IDE's data directory, and a small state file with
//! which of them are switched on. Installing downloads into a temporary folder, checks every file against the
//! registry's index and only then puts the module in place, so a failed download never leaves half a module.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manifest::Manifest;
use super::registry::{IndexEntry, Source, is_plain_name};
use super::sha256::sha256_hex;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct State {
    /// Module id -> switched on. A module that is not listed is on.
    pub enabled: BTreeMap<String, bool>,
    /// The first-run choice of modules has been made (so it is not asked or made again).
    pub seeded: bool,
}

#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub enabled: bool,
}

impl Installed {
    pub fn cover_path(&self) -> Option<PathBuf> {
        Some(self.manifest.cover.as_str()).filter(|c| is_plain_name(c)).map(|c| self.dir.join(c)).filter(|p| p.is_file())
    }
}

pub struct ModuleStore {
    dir: PathBuf,
}

/// What an install is doing: (files done, files in all, the file being fetched).
pub type Progress<'a> = &'a mut dyn FnMut(usize, usize, &str);

impl ModuleStore {
    pub fn at(dir: impl Into<PathBuf>) -> ModuleStore {
        ModuleStore { dir: dir.into() }
    }

    /// `LIGHT_MODULES_DIR`, else `modules` next to the settings.
    pub fn default_dir() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("LIGHT_MODULES_DIR") {
            return Some(PathBuf::from(p));
        }
        crate::settings::Settings::config_dir().map(|d| d.join("modules"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn state(&self) -> State {
        fs::read_to_string(self.state_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn has_state(&self) -> bool {
        self.state_path().is_file()
    }

    pub fn save_state(&self, s: &State) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.dir.join("state.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        fs::rename(&tmp, self.state_path()).map_err(|e| e.to_string())
    }

    pub fn set_enabled(&self, id: &str, on: bool) -> Result<(), String> {
        let mut s = self.state();
        s.enabled.insert(id.to_string(), on);
        self.save_state(&s)
    }

    /// Every installed module (sorted by name), and a message for each folder that holds a broken one.
    pub fn installed_with_errors(&self) -> (Vec<Installed>, Vec<String>) {
        let state = self.state();
        let (mut ok, mut bad) = (Vec::new(), Vec::new());
        let Ok(rd) = fs::read_dir(&self.dir) else { return (ok, bad) };
        for e in rd.flatten() {
            let dir = e.path();
            if !dir.is_dir() || e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let id = e.file_name().to_string_lossy().into_owned();
            match fs::read_to_string(dir.join("module.json")).map_err(|e| e.to_string()).and_then(|j| Manifest::parse(&j)) {
                Ok(m) if m.id == id => {
                    let enabled = state.enabled.get(&id).copied().unwrap_or(true);
                    ok.push(Installed { manifest: m, dir, enabled });
                }
                Ok(m) => bad.push(format!("{id}: the folder holds the module {:?}", m.id)),
                Err(e) => bad.push(format!("{id}: {e}")),
            }
        }
        ok.sort_by_key(|a| a.manifest.name.to_lowercase());
        (ok, bad)
    }

    pub fn installed(&self) -> Vec<Installed> {
        self.installed_with_errors().0
    }

    pub fn get(&self, id: &str) -> Option<Installed> {
        self.installed().into_iter().find(|m| m.manifest.id == id)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        if !is_plain_name(id) {
            return Err(format!("{id}: not a module id"));
        }
        let dir = self.dir.join(id);
        if dir.is_dir() {
            fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut s = self.state();
        s.enabled.remove(id);
        self.save_state(&s)
    }

    /// Install (or update) the module of `entry` from `source`. The module is switched on unless it was switched off
    /// before.
    pub fn install(&self, entry: &IndexEntry, source: &Source, progress: Progress) -> Result<Manifest, String> {
        let source = entry.source.as_ref().unwrap_or(source);
        self.install_from(entry, |tmp| {
            if let Some(archive) = &entry.archive && matches!(source, Source::Url(_)) {
                if !archive.url.starts_with("https://") { return Err("module archives require HTTPS".into()); }
                progress(0, entry.files.len(), "Downloading package");
                let bytes = super::registry::download(&archive.url, super::registry::MAX_PACKAGE)?;
                self.unpack_package(tmp, entry, &bytes, progress)
            } else {
                self.fetch_into(tmp, entry, source, progress)
            }
        })
    }

    /// Install an already downloaded ZIP against trusted catalog metadata.
    pub fn install_archive(&self, entry: &IndexEntry, bytes: &[u8], progress: Progress) -> Result<Manifest, String> {
        self.install_from(entry, |tmp| self.unpack_package(tmp, entry, bytes, progress))
    }

    fn install_from(&self, entry: &IndexEntry, fetch: impl FnOnce(&Path) -> Result<(), String>) -> Result<Manifest, String> {
        if !entry.compatible() {
            return Err(format!("{} requires Light {} or newer", entry.id, entry.min_ide));
        }
        if !is_plain_name(&entry.id) {
            return Err(format!("{}: not a module id", entry.id));
        }
        if !entry.files.iter().any(|f| f.name == "module.json") {
            return Err(format!("{}: the registry lists no module.json", entry.id));
        }
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.dir.join(format!(".installing-{}", entry.id));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let result = fetch(&tmp).and_then(|()| {
            let m = Manifest::parse(&fs::read_to_string(tmp.join("module.json")).map_err(|e| e.to_string())?)?;
            if m.id != entry.id || m.version != entry.version { return Err("module identity/version does not match the catalog".into()); }
            Ok(())
        });
        if let Err(e) = result {
            let _ = fs::remove_dir_all(&tmp);
            return Err(e);
        }
        self.put_in_place(&entry.id, &tmp)
    }

    fn unpack_package(&self, tmp: &Path, entry: &IndexEntry, bytes: &[u8], progress: Progress) -> Result<(), String> {
        use std::io::{Cursor, Read};
        let metadata = entry.archive.as_ref().ok_or("module has no archive metadata")?;
        if bytes.len() as u64 != metadata.size || bytes.len() as u64 > super::registry::MAX_PACKAGE || !sha256_hex(bytes).eq_ignore_ascii_case(&metadata.sha256) {
            return Err("module archive size/checksum mismatch".into());
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("invalid module ZIP: {e}"))?;
        if zip.len() != entry.files.len() { return Err("module ZIP contains unexpected or missing files".into()); }
        let mut seen = std::collections::HashSet::new();
        let mut total = 0_u64;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
            let name = file.name().to_string();
            if !is_plain_name(&name) || !seen.insert(name.to_lowercase()) || file.is_dir() || file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err("module ZIP contains an unsafe or duplicate file name".into());
            }
            let expected = entry.files.iter().find(|f| f.name == name).ok_or("module ZIP contains a file missing from the catalog")?;
            total = total.checked_add(expected.size).ok_or("module ZIP is too large")?;
            if expected.size > super::registry::MAX_FILE || total > super::registry::MAX_PACKAGE || file.size() != expected.size {
                return Err("module ZIP file size exceeds the catalog or limit".into());
            }
            progress(i, entry.files.len(), &name);
            let mut data = Vec::new();
            file.by_ref().take(expected.size + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
            if data.len() as u64 != expected.size || !sha256_hex(&data).eq_ignore_ascii_case(&expected.sha256) {
                return Err(format!("{name}: module ZIP file checksum mismatch"));
            }
            fs::write(tmp.join(&name), data).map_err(|e| e.to_string())?;
        }
        progress(entry.files.len(), entry.files.len(), "");
        Ok(())
    }

    /// Install a module from files that come with the program (nothing is downloaded).
    pub fn install_bytes(&self, id: &str, files: &[(&str, &[u8])]) -> Result<Manifest, String> {
        if !is_plain_name(id) {
            return Err(format!("{id}: not a module id"));
        }
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.dir.join(format!(".installing-{id}"));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        for (name, bytes) in files {
            if !is_plain_name(name) {
                let _ = fs::remove_dir_all(&tmp);
                return Err(format!("{id}: the file name {name:?} is not allowed"));
            }
            if let Err(e) = fs::write(tmp.join(name), bytes) {
                let _ = fs::remove_dir_all(&tmp);
                return Err(format!("{name}: {e}"));
            }
        }
        self.put_in_place(id, &tmp)
    }

    /// Check the module in `tmp` and move it to its place, keeping what was there if anything goes wrong.
    fn put_in_place(&self, id: &str, tmp: &Path) -> Result<Manifest, String> {
        let manifest = match fs::read_to_string(tmp.join("module.json")).map_err(|e| e.to_string()).and_then(|j| Manifest::parse(&j)) {
            Ok(m) if m.id == id => m,
            Ok(m) => {
                let _ = fs::remove_dir_all(tmp);
                return Err(format!("{id}: the module calls itself {:?}", m.id));
            }
            Err(e) => {
                let _ = fs::remove_dir_all(tmp);
                return Err(format!("{id}: {e}"));
            }
        };
        let (dest, old) = (self.dir.join(id), self.dir.join(format!(".replaced-{id}")));
        let _ = fs::remove_dir_all(&old);
        if dest.exists() {
            fs::rename(&dest, &old).map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        if let Err(e) = fs::rename(tmp, &dest) {
            // put the old one back
            if old.exists() {
                let _ = fs::rename(&old, &dest);
            }
            return Err(format!("{}: {e}", dest.display()));
        }
        let _ = fs::remove_dir_all(&old);
        let mut s = self.state();
        s.enabled.entry(id.to_string()).or_insert(true);
        self.save_state(&s)?;
        Ok(manifest)
    }

    fn fetch_into(&self, tmp: &Path, entry: &IndexEntry, source: &Source, progress: Progress) -> Result<(), String> {
        let total = entry.files.len();
        for (i, f) in entry.files.iter().enumerate() {
            progress(i, total, &f.name);
            if !is_plain_name(&f.name) {
                return Err(format!("{}: the file name {:?} is not allowed", entry.id, f.name));
            }
            let bytes = source.fetch(&format!("{}/{}", entry.path, f.name)).map_err(|e| format!("{}: {e}", f.name))?;
            if bytes.len() as u64 != f.size {
                return Err(format!("{}: {} is {} bytes, the index says {}", entry.id, f.name, bytes.len(), f.size));
            }
            if !sha256_hex(&bytes).eq_ignore_ascii_case(&f.sha256) {
                return Err(format!("{}: {} does not match its checksum - not installed", entry.id, f.name));
            }
            fs::write(tmp.join(&f.name), &bytes).map_err(|e| format!("{}: {e}", f.name))?;
        }
        progress(total, total, "");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::registry::FileEntry;
    use crate::testutil::TempDir;

    /// A registry on disk with one module `demo` and its index entry.
    fn registry(d: &TempDir, version: &str) -> (Source, IndexEntry) {
        let manifest = format!(r#"{{"id":"demo","name":"Demo","version":"{version}","cover":"cover.png","languages":[{{"id":"demo","name":"Demo","extensions":["dm"]}}]}}"#);
        let files = [("module.json", manifest.into_bytes()), ("cover.png", vec![1, 2, 3, 4])];
        let dir = d.path().join("reg/modules/demo");
        fs::create_dir_all(&dir).unwrap();
        let mut entries = Vec::new();
        for (name, bytes) in &files {
            fs::write(dir.join(name), bytes).unwrap();
            entries.push(FileEntry { name: name.to_string(), size: bytes.len() as u64, sha256: sha256_hex(bytes) });
        }
        (Source::Dir(d.path().join("reg")), IndexEntry { id: "demo".into(), name: "Demo".into(), version: version.into(), path: "modules/demo".into(), files: entries, ..IndexEntry::default() })
    }

    fn store(d: &TempDir) -> ModuleStore {
        ModuleStore::at(d.path().join("store"))
    }

    #[test]
    fn bundled_entries_are_installed_from_their_own_origin() {
        let d = TempDir::new("bundled-origin");
        let (source, mut entry) = registry(&d, "1.0.0");
        entry.source = Some(source);
        let unavailable = Source::Dir(d.path().join("missing-online-registry"));
        let module = store(&d).install(&entry, &unavailable, &mut |_, _, _| {}).unwrap();
        assert_eq!(module.id, "demo");
    }

    #[test]
    fn install_enable_disable_remove() {
        let d = TempDir::new("store");
        let (src, entry) = registry(&d, "1.0.0");
        let st = store(&d);
        assert!(st.installed().is_empty() && !st.has_state());
        let mut steps = Vec::new();
        let m = st.install(&entry, &src, &mut |i, n, f| steps.push((i, n, f.to_string()))).unwrap();
        assert_eq!(m.id, "demo");
        assert_eq!(steps.first(), Some(&(0, 2, "module.json".to_string())));
        assert_eq!(steps.last(), Some(&(2, 2, String::new())));
        let got = st.installed();
        assert_eq!(got.len(), 1);
        assert!(got[0].enabled, "a new module is on");
        assert!(got[0].cover_path().is_some_and(|p| p.ends_with("cover.png")));
        assert_eq!(st.get("demo").unwrap().manifest.languages[0].extensions, ["dm"]);
        st.set_enabled("demo", false).unwrap();
        assert!(!st.get("demo").unwrap().enabled);
        // updating keeps it off
        let (src2, entry2) = registry(&d, "1.1.0");
        st.install(&entry2, &src2, &mut |_, _, _| {}).unwrap();
        let again = st.get("demo").unwrap();
        assert_eq!((again.manifest.version.as_str(), again.enabled), ("1.1.0", false));
        st.remove("demo").unwrap();
        assert!(st.installed().is_empty());
        assert!(!st.dir().join("demo").exists());
        // nothing is left behind
        assert!(fs::read_dir(st.dir()).unwrap().flatten().all(|e| e.file_name() == "state.json"));
    }

    fn package(entry: &mut IndexEntry, source: &Source, extra: Option<(&str, &[u8])>) -> Vec<u8> {
        use std::io::{Cursor, Write};
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for file in &entry.files {
            writer.start_file(&file.name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(&source.fetch(&format!("{}/{}", entry.path, file.name)).unwrap()).unwrap();
        }
        if let Some((name, data)) = extra {
            writer.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(data).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        entry.archive = Some(super::super::registry::Archive { url: "https://example.org/demo.zip".into(), size: bytes.len() as u64, sha256: sha256_hex(&bytes) });
        bytes
    }

    #[test]
    fn versioned_zip_updates_are_verified_before_replacing_the_installed_module() {
        let d = TempDir::new("zip-updates");
        let (src, first) = registry(&d, "1.0.0");
        let st = store(&d);
        st.install(&first, &src, &mut |_, _, _| {}).unwrap();
        let (src, mut next) = registry(&d, "1.1.0");
        let bytes = package(&mut next, &src, None);
        let mut corrupt = bytes.clone();
        corrupt[0] ^= 1;
        assert!(st.install_archive(&next, &corrupt, &mut |_, _, _| {}).is_err());
        assert_eq!(st.get("demo").unwrap().manifest.version, "1.0.0");
        let mut wrong_file = next.clone();
        wrong_file.files[0].sha256 = "0".repeat(64);
        assert!(st.install_archive(&wrong_file, &bytes, &mut |_, _, _| {}).is_err());
        assert_eq!(st.get("demo").unwrap().manifest.version, "1.0.0");
        let mut wrong_version = next.clone();
        wrong_version.version = "2.0.0".into();
        assert!(st.install_archive(&wrong_version, &bytes, &mut |_, _, _| {}).is_err());
        next.min_ide = "99.0.0".into();
        assert!(st.install_archive(&next, &bytes, &mut |_, _, _| {}).unwrap_err().contains("requires Light"));
        next.min_ide.clear();
        let installed = st.install_archive(&next, &bytes, &mut |_, _, _| {}).unwrap();
        assert_eq!(installed.version, "1.1.0");
        assert_eq!(st.get("demo").unwrap().manifest.version, "1.1.0");
    }

    #[test]
    fn zip_entries_cannot_escape_or_bypass_the_catalog() {
        let d = TempDir::new("zip-paths");
        let (src, mut entry) = registry(&d, "1.0.0");
        let st = store(&d);
        let bytes = package(&mut entry, &src, Some(("../outside.txt", b"no")));
        assert!(st.install_archive(&entry, &bytes, &mut |_, _, _| {}).is_err());
        assert!(!st.dir().join("outside.txt").exists());
        assert!(st.installed().is_empty());
        let bytes = package(&mut entry, &src, None);
        entry.files[0].size += 1;
        assert!(st.install_archive(&entry, &bytes, &mut |_, _, _| {}).is_err());
        assert!(st.installed().is_empty());
    }

    #[test]
    fn a_corrupt_download_installs_nothing() {
        let d = TempDir::new("corrupt");
        let (src, mut entry) = registry(&d, "1.0.0");
        let st = store(&d);
        // wrong checksum
        entry.files[1].sha256 = sha256_hex(b"something else");
        let e = st.install(&entry, &src, &mut |_, _, _| {}).unwrap_err();
        assert!(e.contains("checksum"), "{e}");
        assert!(st.installed().is_empty() && !st.dir().join("demo").exists());
        assert!(fs::read_dir(st.dir()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".installing")), "the temporary folder is gone");
        // wrong size
        let (_, mut entry) = registry(&d, "1.0.0");
        entry.files[1].size += 1;
        assert!(st.install(&entry, &src, &mut |_, _, _| {}).unwrap_err().contains("bytes"));
        // a file the registry does not have
        let (_, mut entry) = registry(&d, "1.0.0");
        entry.files.push(FileEntry { name: "missing.png".into(), size: 1, sha256: "x".into() });
        assert!(st.install(&entry, &src, &mut |_, _, _| {}).is_err());
        // a name that would leave the module's folder
        let (_, mut entry) = registry(&d, "1.0.0");
        entry.files[1].name = "../evil".into();
        assert!(st.install(&entry, &src, &mut |_, _, _| {}).unwrap_err().contains("not allowed"));
        // no manifest, and a manifest of another module
        let (_, mut entry) = registry(&d, "1.0.0");
        entry.files.remove(0);
        assert!(st.install(&entry, &src, &mut |_, _, _| {}).unwrap_err().contains("module.json"));
        let (_, mut entry) = registry(&d, "1.0.0");
        entry.id = "other".into();
        assert!(st.install(&entry, &src, &mut |_, _, _| {}).is_err());
        assert!(st.installed().is_empty());
    }

    #[test]
    fn a_failed_update_keeps_the_old_module() {
        let d = TempDir::new("keep-old");
        let (src, entry) = registry(&d, "1.0.0");
        let st = store(&d);
        st.install(&entry, &src, &mut |_, _, _| {}).unwrap();
        let (_, mut bad) = registry(&d, "1.0.0");
        bad.version = "2.0.0".into();
        bad.files[0].sha256 = "0".repeat(64);
        assert!(st.install(&bad, &src, &mut |_, _, _| {}).is_err());
        assert_eq!(st.get("demo").unwrap().manifest.version, "1.0.0");
    }

    #[test]
    fn broken_folders_are_reported_not_loaded() {
        let d = TempDir::new("broken");
        let st = store(&d);
        fs::create_dir_all(st.dir().join("junk")).unwrap();
        fs::write(st.dir().join("junk/module.json"), "{ nope").unwrap();
        fs::create_dir_all(st.dir().join("wrong")).unwrap();
        fs::write(st.dir().join("wrong/module.json"), r#"{"id":"right","name":"R","version":"1.0.0"}"#).unwrap();
        fs::create_dir_all(st.dir().join(".installing-x")).unwrap();
        let (ok, bad) = st.installed_with_errors();
        assert!(ok.is_empty());
        assert_eq!(bad.len(), 2, "{bad:?}");
        assert!(st.remove("../x").is_err());
    }

    #[test]
    fn modules_that_come_with_the_program_install_without_a_registry() {
        let d = TempDir::new("bytes");
        let st = store(&d);
        let m = st.install_bytes("lisp", &[("module.json", br#"{"id":"lisp","name":"Lisp","version":"1.0.0","builtin":"lisp"}"#), ("cover.png", &[1, 2])]).unwrap();
        assert_eq!(m.builtin, "lisp");
        assert!(st.get("lisp").unwrap().enabled);
        // wrong id, bad file names and bad manifests leave nothing behind
        assert!(st.install_bytes("other", &[("module.json", br#"{"id":"lisp","name":"L","version":"1.0.0"}"#)]).is_err());
        assert!(st.install_bytes("x", &[("../module.json", b"{}")]).is_err());
        assert!(st.install_bytes("y", &[("module.json", b"nope")]).is_err());
        assert_eq!(st.installed().len(), 1);
        assert!(fs::read_dir(st.dir()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".installing")));
    }
}

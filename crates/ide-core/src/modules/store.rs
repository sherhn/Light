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
        if let Ok(p) = std::env::var("LIGHT_MODULES_DIR").or_else(|_| std::env::var("AIR_MODULES_DIR")) {
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
        let result = self.fetch_into(&tmp, entry, source, progress);
        if let Err(e) = result {
            let _ = fs::remove_dir_all(&tmp);
            return Err(e);
        }
        self.put_in_place(&entry.id, &tmp)
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

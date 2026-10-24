//! Where modules come from: a registry is a folder (on a web server such as raw.githubusercontent.com, or on disk)
//! with an `index.json` and one sub-folder per module. The index lists every file of every module with its size and
//! SHA-256, so what arrives can be checked before it is used.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use super::manifest::Detect;

/// Where the modules of this release are published (the `registry` folder of the repository).
pub const DEFAULT_URL: &str = "https://raw.githubusercontent.com/sherhn/Light/main/registry";

/// Largest file a module may have, in bytes.
pub const MAX_FILE: u64 = 20 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct IndexEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub category: String,
    pub summary: String,
    pub cover: String,
    /// The module's folder, relative to the registry.
    pub path: String,
    pub files: Vec<FileEntry>,
    pub detect: Detect,
}

impl IndexEntry {
    /// Bytes to download.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Index {
    pub version: u32,
    pub modules: Vec<IndexEntry>,
}

impl Index {
    pub fn parse(json: &str) -> Result<Index, String> {
        let i: Index = serde_json::from_str(json).map_err(|e| format!("index.json: {e}"))?;
        if i.version == 0 {
            return Err("index.json: this is not a module index (it has no version)".into());
        }
        for m in &i.modules {
            if !is_plain_name(&m.id) || m.path.split('/').any(|p| p == ".." || p.is_empty()) || m.path.contains('\\') {
                return Err(format!("index.json: the module {:?} has a path that is not allowed", m.id));
            }
            for f in &m.files {
                if !is_plain_name(&f.name) {
                    return Err(format!("index.json: {}: the file name {:?} is not allowed", m.id, f.name));
                }
            }
        }
        Ok(i)
    }

    pub fn entry(&self, id: &str) -> Option<&IndexEntry> {
        self.modules.iter().find(|m| m.id == id)
    }
}

/// A file name that stays inside the module's folder.
pub fn is_plain_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 80 && !n.starts_with('.') && !n.contains(['/', '\\', ':']) && n != ".." && n.chars().all(|c| !c.is_control())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Url(String),
    Dir(PathBuf),
}

impl Source {
    /// `https://...` is a web registry, anything else (also `file://...`) a folder.
    pub fn parse(s: &str) -> Source {
        let s = s.trim();
        if s.starts_with("http://") || s.starts_with("https://") {
            Source::Url(s.trim_end_matches('/').to_string())
        } else {
            Source::Dir(PathBuf::from(s.strip_prefix("file://").unwrap_or(s)))
        }
    }

    pub fn label(&self) -> String {
        match self {
            Source::Url(u) => u.clone(),
            Source::Dir(d) => d.display().to_string(),
        }
    }

    /// The bytes of `rel` (a path of `/`-separated plain names) in this registry.
    pub fn fetch(&self, rel: &str) -> Result<Vec<u8>, String> {
        if rel.split('/').any(|p| p.is_empty() || p == "." || p == ".." || p.contains(['\\', ':'])) {
            return Err(format!("{rel}: not a path inside a registry"));
        }
        match self {
            Source::Url(base) => curl_get(&format!("{base}/{rel}")),
            Source::Dir(dir) => {
                let path = rel.split('/').fold(dir.clone(), |p, part| p.join(part));
                let len = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
                if len > MAX_FILE {
                    return Err(format!("{}: too large", path.display()));
                }
                std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
            }
        }
    }

    pub fn fetch_index(&self) -> Result<Index, String> {
        let bytes = self.fetch("index.json")?;
        Index::parse(&String::from_utf8_lossy(&bytes))
    }
}

/// The first of `sources` that has an index: the index and where it came from.
pub fn fetch_index(sources: &[Source]) -> Result<(Index, Source), String> {
    let mut errors = Vec::new();
    for s in sources {
        match s.fetch_index() {
            Ok(i) => return Ok((i, s.clone())),
            Err(e) => errors.push(format!("{}: {e}", s.label())),
        }
    }
    Err(if errors.is_empty() { "no registry is set".into() } else { errors.join("; ") })
}

/// A file from the web with the system's `curl`.
fn curl_get(url: &str) -> Result<Vec<u8>, String> {
    let mut c = Command::new("curl");
    c.args(["-fsSL", "--connect-timeout", "15", "--max-time", "180", "--max-filesize", &MAX_FILE.to_string(), "-A", "Light", url]);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = c.output().map_err(|e| format!("cannot run curl: {e}"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if why.is_empty() { format!("{url}: download failed") } else { why });
    }
    Ok(out.stdout)
}

/// The registries to try, best first: the one set in `LIGHT_REGISTRY` or in the settings, the published one, and the
/// copy that is shipped next to the program (so modules can be added without a network).
pub fn default_sources(custom: &str, bundled: Option<&Path>) -> Vec<Source> {
    let mut v = Vec::new();
    if let Ok(s) = std::env::var("LIGHT_REGISTRY").or_else(|_| std::env::var("AIR_REGISTRY"))
        && !s.trim().is_empty()
    {
        v.push(Source::parse(&s));
    } else if !custom.trim().is_empty() {
        v.push(Source::parse(custom));
    }
    if v.is_empty() {
        v.push(Source::Url(DEFAULT_URL.to_string()));
    }
    if let Some(b) = bundled.filter(|b| b.join("index.json").is_file()) {
        v.push(Source::Dir(b.to_path_buf()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_are_told_apart() {
        assert_eq!(Source::parse("https://example.org/reg/"), Source::Url("https://example.org/reg".into()));
        assert_eq!(Source::parse("file:///tmp/reg"), Source::Dir(PathBuf::from("/tmp/reg")));
        assert_eq!(Source::parse(r"C:\reg"), Source::Dir(PathBuf::from(r"C:\reg")));
    }

    #[test]
    fn the_index_refuses_paths_that_leave_the_registry() {
        assert!(Index::parse(r#"{"version":1,"modules":[{"id":"a","path":"modules/a","files":[{"name":"module.json","size":1,"sha256":"x"}]}]}"#).is_ok());
        for bad in [
            r#"{"modules":[{"id":"a","path":"../etc","files":[]}]}"#,
            r#"{"modules":[{"id":"a","path":"/abs","files":[]}]}"#,
            r#"{"modules":[{"id":"a","path":"m\\a","files":[]}]}"#,
            r#"{"modules":[{"id":"a/b","path":"m","files":[]}]}"#,
            r#"{"modules":[{"id":"a","path":"m","files":[{"name":"../x","size":1,"sha256":"y"}]}]}"#,
            r#"{"modules":[{"id":"a","path":"m","files":[{"name":"C:x","size":1,"sha256":"y"}]}]}"#,
        ] {
            assert!(Index::parse(bad).is_err(), "{bad}");
        }
        assert!(Index::parse("[]").is_err());
        assert!(!is_plain_name(".hidden") && !is_plain_name("a/b") && !is_plain_name("") && is_plain_name("cover.png"));
    }

    #[test]
    fn a_folder_registry_serves_files_and_refuses_escapes() {
        let d = crate::testutil::TempDir::new("registry");
        std::fs::create_dir_all(d.path().join("modules/a")).unwrap();
        std::fs::write(d.path().join("modules/a/module.json"), "{}").unwrap();
        std::fs::write(d.path().join("index.json"), r#"{"version":1,"modules":[]}"#).unwrap();
        let s = Source::Dir(d.path().to_path_buf());
        assert_eq!(s.fetch("modules/a/module.json").unwrap(), b"{}");
        assert!(s.fetch("../x").is_err() && s.fetch("modules//a").is_err() && s.fetch("modules/a/missing").is_err());
        assert_eq!(s.fetch_index().unwrap().version, 1);
        let (_, from) = fetch_index(&[Source::Dir(d.path().join("nowhere")), s.clone()]).unwrap();
        assert_eq!(from, s, "the first registry that answers is used");
        assert!(fetch_index(&[]).is_err());
    }

    #[test]
    fn the_registries_to_try() {
        // (LIGHT_REGISTRY is not set by the tests)
        let v = default_sources("", None);
        assert_eq!(v, [Source::Url(DEFAULT_URL.into())]);
        let d = crate::testutil::TempDir::new("bundled");
        assert_eq!(default_sources("https://my.example/reg", Some(d.path())), [Source::Url("https://my.example/reg".into())], "no index there: not offered");
        std::fs::write(d.path().join("index.json"), "{}").unwrap();
        assert_eq!(default_sources("/some/dir", Some(d.path())), [Source::Dir("/some/dir".into()), Source::Dir(d.path().into())]);
    }
}

//! The languages that are available: plain text always, and one for every language a module brings.

use std::path::Path;
use std::sync::Arc;

use crate::def::LangDef;
use crate::engine::Language;

#[derive(Clone)]
pub struct LanguageSet {
    langs: Vec<Arc<Language>>,
    plain: Arc<Language>,
}

impl Default for LanguageSet {
    fn default() -> Self {
        LanguageSet { langs: Vec::new(), plain: Arc::new(Language::plain()) }
    }
}

impl LanguageSet {
    pub fn new() -> LanguageSet {
        LanguageSet::default()
    }

    /// Add a language (one of the same id is replaced).
    pub fn add(&mut self, def: LangDef) -> Result<(), String> {
        let lang = Arc::new(Language::new(def)?);
        self.langs.retain(|l| l.id() != lang.id());
        self.langs.push(lang);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) {
        self.langs.retain(|l| l.id() != id);
    }

    pub fn clear(&mut self) {
        self.langs.clear();
    }

    pub fn all(&self) -> &[Arc<Language>] {
        &self.langs
    }

    pub fn plain(&self) -> Arc<Language> {
        self.plain.clone()
    }

    pub fn by_id(&self, id: &str) -> Option<Arc<Language>> {
        self.langs.iter().find(|l| l.id() == id).cloned()
    }

    /// The language of a file: by its name, then its extension, then its first line; plain text if nothing claims it.
    pub fn for_path(&self, path: &Path, first_line: Option<&str>) -> Arc<Language> {
        self.find(path, first_line).unwrap_or_else(|| self.plain.clone())
    }

    /// Like [`for_path`](Self::for_path), `None` when no module claims the file.
    pub fn find(&self, path: &Path, first_line: Option<&str>) -> Option<Arc<Language>> {
        let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        self.langs
            .iter()
            .find(|l| l.def.filenames.iter().any(|f| f.eq_ignore_ascii_case(&name)))
            .or_else(|| if ext.is_empty() { None } else { self.langs.iter().find(|l| l.def.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext))) })
            .or_else(|| first_line.and_then(|f| self.langs.iter().find(|l| l.matches_first_line(f))))
            .cloned()
    }
}

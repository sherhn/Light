//! The languages that are available: plain text always, and one for every language a module brings.

use std::path::Path;
use std::sync::Arc;

use crate::def::LangDef;
use crate::engine::Language;

/// Case-insensitive filename patterns; `*` matches any sequence of characters.
pub fn matches_file_name(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let parts: Vec<_> = pattern.split('*').collect();
    if parts.len() == 1 { return pattern == name }
    let Some(mut remaining) = name.strip_prefix(parts[0]) else { return false };
    for part in &parts[1..parts.len() - 1] {
        let Some(offset) = remaining.find(part) else { return false };
        remaining = &remaining[offset + part.len()..];
    }
    remaining.ends_with(parts[parts.len() - 1])
}

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
            .find(|l| l.def.filenames.iter().any(|f| !f.contains('*') && f.eq_ignore_ascii_case(&name)))
            .or_else(|| self.langs.iter().flat_map(|language| language.def.filenames.iter().filter(|pattern| pattern.contains('*') && matches_file_name(pattern, &name)).map(move |pattern| (pattern.chars().filter(|c| *c != '*').count(), language))).max_by_key(|(specificity, _)| *specificity).map(|(_, language)| language))
            .or_else(|| if ext.is_empty() { None } else { self.langs.iter().find(|l| l.def.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext))) })
            .or_else(|| first_line.and_then(|f| self.langs.iter().find(|l| l.matches_first_line(f))))
            .cloned()
    }
}

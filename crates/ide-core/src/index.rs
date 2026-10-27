//! Project wide symbol index: Go to Symbol, Go to Declaration, usage search.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lisp_syntax::outline::{Definition, outline};
use lisp_syntax::{Dialect, symbols};

use crate::search::{self, FileHit, FuzzyMatch};

#[derive(Clone, Debug)]
pub struct IndexedSymbol {
    pub path: PathBuf,
    pub def: Definition,
}

/// Flattened, sorted view of the index (built lazily, cached until the next update).
pub struct Flat {
    /// Sorted by lowercase name.
    pub symbols: Vec<IndexedSymbol>,
    /// Lowercase full names, parallel to `symbols`.
    pub lc: Vec<String>,
    by_key: HashMap<String, Vec<usize>>,
}

#[derive(Default)]
pub struct ProjectIndex {
    by_file: HashMap<PathBuf, Vec<Definition>>,
    cache: Mutex<Option<Arc<Flat>>>,
}

impl ProjectIndex {
    pub fn build(root: &Path) -> ProjectIndex {
        ProjectIndex::build_from(search::walk_files(root))
    }

    /// The index of these files (the project's, without what it hides).
    pub fn build_from(files: impl IntoIterator<Item = PathBuf>) -> ProjectIndex {
        let mut idx = ProjectIndex::default();
        for p in files {
            idx.update_file_from_disk(&p);
        }
        idx
    }

    fn invalidate(&self) {
        *self.cache.lock().unwrap() = None;
    }

    pub fn update_file_from_disk(&mut self, path: &Path) {
        let Some(d) = Dialect::from_path(path) else { return };
        match fs::metadata(path) {
            Ok(m) if m.len() > 4 * 1024 * 1024 => return, // generated / data files are not worth indexing
            _ => {}
        }
        match fs::read_to_string(path) {
            Ok(text) => self.update_file(path, &text, d),
            Err(_) => {
                if self.by_file.remove(path).is_some() {
                    self.invalidate();
                }
            }
        }
    }

    pub fn update_file(&mut self, path: &Path, text: &str, d: Dialect) {
        let defs = outline(text, d);
        let changed = if defs.is_empty() { self.by_file.remove(path).is_some() } else { { self.by_file.insert(path.to_path_buf(), defs); true } };
        if changed {
            self.invalidate();
        }
    }

    pub fn remove_file(&mut self, path: &Path) {
        if self.by_file.remove(path).is_some() {
            self.invalidate();
        }
    }

    pub fn symbol_count(&self) -> usize {
        self.by_file.values().flatten().filter(|d| !d.operator.eq_ignore_ascii_case("in-package")).count()
    }

    /// All definitions, sorted by name (cached; cheap to call repeatedly).
    pub fn all(&self) -> Arc<Flat> {
        let mut guard = self.cache.lock().unwrap();
        if let Some(f) = &*guard {
            return f.clone();
        }
        let mut v: Vec<(String, IndexedSymbol)> = self
            .by_file
            .iter()
            .flat_map(|(p, ds)| ds.iter().filter(|d| !d.operator.eq_ignore_ascii_case("in-package")).map(|d| (d.name.to_lowercase(), IndexedSymbol { path: p.clone(), def: d.clone() })))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.path.cmp(&b.1.path)).then(a.1.def.start.cmp(&b.1.def.start)));
        let mut lc = Vec::with_capacity(v.len());
        let mut symbols = Vec::with_capacity(v.len());
        let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, (l, sym)) in v.into_iter().enumerate() {
            by_key.entry(normalize(sym.def.name.split(' ').next().unwrap_or(""))).or_default().push(i);
            lc.push(l);
            symbols.push(sym);
        }
        let flat = Arc::new(Flat { symbols, lc, by_key });
        *guard = Some(flat.clone());
        flat
    }

    /// Definitions of `name` (package prefixes and case are ignored, as in Lisp).
    pub fn find_definitions(&self, name: &str) -> Vec<IndexedSymbol> {
        let flat = self.all();
        flat.by_key.get(&normalize(name)).map(|ix| ix.iter().map(|&i| flat.symbols[i].clone()).collect()).unwrap_or_default()
    }

    /// Up to `limit` symbols whose lowercase name starts with `prefix_lc` (binary search on the sorted names).
    pub fn with_prefix(&self, prefix_lc: &str, limit: usize) -> Vec<IndexedSymbol> {
        let flat = self.all();
        let start = flat.lc.partition_point(|n| n.as_str() < prefix_lc);
        flat.lc[start..].iter().zip(&flat.symbols[start..]).take_while(|(n, _)| n.starts_with(prefix_lc)).take(limit).map(|(_, s)| s.clone()).collect()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<(IndexedSymbol, FuzzyMatch)> {
        let flat = self.all();
        let mut hits: Vec<(usize, FuzzyMatch)> = Vec::new();
        for (i, s) in flat.symbols.iter().enumerate() {
            if let Some(m) = search::fuzzy_match(query, &s.def.name) {
                hits.push((i, m));
            }
        }
        hits.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
        hits.truncate(limit);
        hits.into_iter().map(|(i, m)| (flat.symbols[i].clone(), m)).collect()
    }
}

fn normalize(name: &str) -> String {
    symbols::strip_package(name).trim_start_matches(['#', '\'', ':']).to_ascii_lowercase()
}

/// Byte ranges of every code occurrence of the symbol `name` in `text` (strings and comments are
/// ignored; a package prefix is kept out of the range: `pkg:foo` -> the `foo` part).
pub fn symbol_ranges(text: &str, d: Dialect, name: &str) -> Vec<(usize, usize)> {
    use lisp_syntax::TokenKind;
    let want = normalize(name);
    if want.is_empty() {
        return vec![];
    }
    let p = lisp_syntax::parse(text, d);
    let mut out = Vec::new();
    for t in &p.tokens {
        if !matches!(t.kind, TokenKind::Symbol | TokenKind::Keyword) {
            continue;
        }
        let tok = t.text(text);
        // `#:foo` / `:foo` / `pkg:foo` / `pkg::foo`
        let name_start = match tok.rfind(':') {
            Some(i) if i + 1 < tok.len() => i + 1,
            _ => 0,
        };
        let part = &tok[name_start..];
        if part.eq_ignore_ascii_case(&want) || part.to_ascii_lowercase() == want {
            out.push((t.start + name_start, t.end));
        }
    }
    out
}

/// Usages of `name` across the project's Lisp files (token based, so strings/comments don't count).
pub fn find_usages(root: &Path, name: &str, limit: usize) -> Vec<FileHit> {
    let mut hits = Vec::new();
    for path in search::walk_files(root) {
        let Some(d) = Dialect::from_path(&path) else { continue };
        let Ok(text) = fs::read_to_string(&path) else { continue };
        for (s, e) in symbol_ranges(&text, d, name) {
            let line_start = text[..s].rfind('\n').map_or(0, |i| i + 1);
            let line_end = text[s..].find('\n').map_or(text.len(), |i| s + i);
            hits.push(FileHit {
                path: path.clone(),
                line: text[..s].matches('\n').count(),
                col: text[line_start..s].chars().count(),
                len: text[s..e].chars().count(),
                line_text: text[line_start..line_end].trim_end().to_string(),
            });
            if hits.len() >= limit {
                return hits;
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn project() -> TempDir {
        let d = TempDir::new("index");
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(d.path().join("src/a.lisp"), "(in-package :a)\n(defun greet (n) n)\n(defvar *x* 1)\n").unwrap();
        fs::write(d.path().join("src/b.lisp"), "(defun other () (greet 1))\n(defmethod greet ((x string)) x)\n").unwrap();
        fs::write(d.path().join("notes.txt"), "greet").unwrap();
        d
    }

    #[test]
    fn builds_and_finds_definitions() {
        let d = project();
        let idx = ProjectIndex::build(d.path());
        assert_eq!(idx.symbol_count(), 4);
        let defs = idx.find_definitions("GREET");
        assert_eq!(defs.len(), 2);
        assert!(defs.iter().any(|s| s.path.ends_with("a.lisp")));
        assert_eq!(idx.find_definitions("pkg:greet").len(), 2, "package prefix ignored");
        assert!(idx.find_definitions("missing").is_empty());
    }

    #[test]
    fn fuzzy_symbol_search() {
        let d = project();
        let idx = ProjectIndex::build(d.path());
        let r = idx.search("gr", 5);
        assert!(r[0].0.def.name.starts_with("greet"));
        assert!(idx.search("zzzz", 5).is_empty());
    }

    #[test]
    fn incremental_update() {
        let d = project();
        let mut idx = ProjectIndex::build(d.path());
        let a = d.path().join("src/a.lisp");
        idx.update_file(&a, "(defun renamed ())", Dialect::CommonLisp);
        assert!(idx.find_definitions("renamed").len() == 1);
        assert_eq!(idx.find_definitions("greet").len(), 1);
        idx.remove_file(&a);
        assert!(idx.find_definitions("renamed").is_empty());
    }

    #[test]
    fn usages_are_token_based() {
        let d = project();
        let hits = find_usages(d.path(), "greet", 100);
        // a.lisp defun, b.lisp call, b.lisp defmethod; notes.txt is not a Lisp file
        assert_eq!(hits.len(), 3, "{hits:#?}");
        fs::write(d.path().join("src/c.lisp"), "(greeting) \"greet\" ; greet
(pkg:greet) #'greet").unwrap();
        let hits = find_usages(d.path(), "greet", 100);
        assert_eq!(hits.len(), 5, "string and comment occurrences are not usages: {hits:#?}");
    }

    #[test]
    fn symbol_ranges_handle_prefixes_and_case() {
        let t = "(Foo :foo pkg:FOO pkg::foo #:foo 'foo foo-bar \"foo\" ; foo
)";
        let r = symbol_ranges(t, Dialect::CommonLisp, "foo");
        let got: Vec<&str> = r.iter().map(|&(s, e)| &t[s..e]).collect();
        assert_eq!(got, vec!["Foo", "foo", "FOO", "foo", "foo", "foo"]);
        assert!(symbol_ranges(t, Dialect::CommonLisp, "").is_empty());
    }
}

//! The documentation popup: rest the pointer on a name and see what it is - its signature (the arguments) and what
//! its author wrote about it. Lisp names come from the project's own definitions or, when the REPL runs, from the
//! image itself; names of the languages modules bring come from the definition found in the project (its header and
//! the docstring or the comments around it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use light_lang::LanguageSet;
use eframe::egui::{self, Align2, Color32, FontId, Margin, Pos2, Rect, RichText, Sense, pos2, vec2};
use ide_core::docinfo;
use lisp_syntax::highlight::Class;

use crate::app::App;
use crate::editor_view::ViewOut;
use crate::fonts;
use crate::state::Pending;

/// How long the pointer rests on a name before the popup comes.
const DELAY: f64 = 0.45;
/// How long an answer from the REPL is kept.
const CACHE_SECS: f64 = 30.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoverInfo {
    pub title: String,
    /// `function`, `macro`, `variable`, `class`...
    pub kind: String,
    /// The call as written: `(greet name &optional greeting)`, `def greet(name):`.
    pub signature: String,
    pub doc: String,
    /// Where it comes from: `src/main.lisp:12`, `Common Lisp image`.
    pub source: String,
    /// A stand-in until something better is known (the REPL is not running): not kept.
    pub fallback: bool,
}

/// A definition of a name in a file of the project.
#[derive(Clone, Debug)]
pub struct DefEntry {
    pub path: PathBuf,
    pub line: usize,
    pub kind: String,
    /// The id of the language of the file.
    pub lang: String,
}

/// A definition found for a name: where it is, and what it says about itself.
#[derive(Clone, Debug)]
pub struct LangDefHit {
    /// The name as the definition spells it (it may differ from the call's: `_name` in assembly, `name-x` in Lisp).
    pub name: String,
    pub path: PathBuf,
    pub kind: String,
    /// Byte offset of the name in the file.
    pub offset: usize,
    pub doc: docinfo::DocInfo,
    /// `portable/file.c:35`.
    pub source: String,
}

/// The names a call of `name` may be defined under in another language: the C symbol with an underscore put in front
/// (assembly) or taken off, and Lisp's dashes for underscores and back. The first is `name` itself.
pub fn name_variants(name: &str) -> Vec<String> {
    let mut v = vec![name.to_string()];
    let mut add = |s: String| {
        if !s.is_empty() && !v.contains(&s) {
            v.push(s);
        }
    };
    match name.strip_prefix('_') {
        Some(rest) => add(rest.to_string()),
        None => add(format!("_{name}")),
    }
    add(name.replace('_', "-"));
    add(name.replace('-', "_"));
    v
}

/// The uses of any of `names` in `text` that are code (not comments or strings), as the hits of a search. `spans` are
/// the coloured spans of the text (comments and strings are skipped); an empty list skips nothing.
pub fn usages_in(path: &Path, text: &str, names: &[String], spans: &[lisp_syntax::highlight::Span]) -> Vec<ide_core::search::FileHit> {
    let dead = |at: usize| {
        let i = spans.partition_point(|s| s.end <= at);
        spans.get(i).is_some_and(|s| s.start <= at && matches!(s.class, Class::Comment | Class::String | Class::Char))
    };
    let id = |c: char| c.is_alphanumeric() || c == '_';
    let mut hits = vec![];
    for name in names {
        for (at, _) in text.match_indices(name.as_str()) {
            let end = at + name.len();
            // a name with a dash is Lisp's: its neighbours may be anything but a symbol character
            let sym = |c: char| id(c) || c == '-';
            let bounded = if name.contains('-') { text[..at].chars().next_back().is_none_or(|c| !sym(c)) && text[end..].chars().next().is_none_or(|c| !sym(c)) } else { text[..at].chars().next_back().is_none_or(|c| !id(c)) && text[end..].chars().next().is_none_or(|c| !id(c)) };
            if !bounded || dead(at) {
                continue;
            }
            let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
            let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
            hits.push(ide_core::search::FileHit {
                path: path.to_path_buf(),
                line: text[..at].matches('\n').count(),
                col: text[line_start..at].chars().count(),
                len: name.chars().count(),
                line_text: text[line_start..line_end].trim_end().to_string(),
            });
        }
    }
    hits.sort_by_key(|h| (h.line, h.col));
    hits.dedup_by_key(|h| (h.line, h.col));
    hits
}

/// The definitions of a file in a language of a module, as the index keeps them: (name, where).
pub fn file_definitions(path: &Path, text: &str, lang: &light_lang::Language) -> Vec<(String, DefEntry)> {
    lang.outline(text).into_iter().map(|d| (d.name, DefEntry { path: path.to_path_buf(), line: d.line, kind: d.kind.label().to_string(), lang: lang.id().to_string() })).collect()
}

/// Is `path` read by the index of the languages of modules (not Lisp, which has its own, nor plain text or documents)?
fn indexed_language(path: &Path, langs: &LanguageSet) -> Option<Arc<light_lang::Language>> {
    let lang = langs.find(path, None)?;
    (!lang.is_plain() && crate::preview::markup_of(&lang).is_none() && !lisp_syntax::dialect::is_lisp_file(path)).then_some(lang)
}

/// Files whose text is data or prose, not code: a definition is not looked for in them.
const NOT_CODE: [&str; 14] = ["md", "markdown", "rst", "txt", "json", "lock", "csv", "tsv", "svg", "log", "xml", "map", "min.js", "sum"];

/// Read `files` for the definitions of `name` that `finder` recognises (see [`ide_core::defsearch`]): comments and
/// strings are skipped where the language is known, the open files are read as they are in the editor. The files of
/// the language of `near` (by extension) and those beside it come first.
pub fn definitions_in_text(finder: &ide_core::defsearch::DefFinder, name: &str, files: Vec<PathBuf>, open: &[(PathBuf, String)], root: &Path, langs: &LanguageSet, near: Option<&Path>) -> Vec<LangDefHit> {
    let ext = |p: &Path| p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let (near_ext, near_dir) = (near.map(ext), near.and_then(Path::parent));
    let mut files: Vec<PathBuf> = files.into_iter().take(40_000).filter(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        !NOT_CODE.iter().any(|x| n.ends_with(&format!(".{x}")))
    }).collect();
    files.sort_by_cached_key(|p| (near_ext.as_ref() != Some(&ext(p)), near_dir != p.parent(), p.clone()));
    let mut out: Vec<LangDefHit> = vec![];
    for path in files {
        let text = match open.iter().find(|(p, _)| *p == path) {
            Some((_, t)) => t.clone(),
            None => match std::fs::metadata(&path) {
                Ok(m) if m.len() <= 2_000_000 => match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
                _ => continue,
            },
        };
        // a cheap look first: most files do not even mention the name
        if !text.contains(name) {
            continue;
        }
        let lang = langs.find(&path, None).filter(|l| !l.is_plain());
        let spans: Vec<lisp_syntax::highlight::Span> = match &lang {
            Some(l) => l.highlight(&text),
            None if lisp_syntax::dialect::is_lisp_file(&path) => {
                let d = lisp_syntax::Dialect::from_path(&path).unwrap_or_default();
                lisp_syntax::highlight::highlight(&text, &lisp_syntax::parse(&text, d))
            }
            None => vec![],
        };
        let dead = |at: usize| {
            let i = spans.partition_point(|s| s.end <= at);
            spans.get(i).is_some_and(|s| s.start <= at && matches!(s.class, Class::Comment | Class::String | Class::Char))
        };
        for d in finder.find(&text, dead) {
            let doc = lang.as_ref().map(|l| docinfo::definition_doc(&text, d.line, l)).unwrap_or_default();
            let source = if path.starts_with(root) { format!("{}:{}", ide_core::workspace::relative(root, &path), d.line + 1) } else { format!("{}:{}", path.display(), d.line + 1) };
            out.push(LangDefHit { name: name.to_string(), path: path.clone(), kind: d.kind.to_string(), offset: d.offset, doc, source });
        }
        if out.len() >= 30 {
            break;
        }
    }
    out.truncate(30);
    out
}

/// Name -> where it is defined.
type Names = HashMap<String, Vec<DefEntry>>;
/// File -> the names it defines.
type Files = HashMap<PathBuf, Vec<String>>;

/// The definitions of the whole project, whatever the language: name -> where they are.
#[derive(Default)]
pub struct LangIndex {
    map: Names,
    /// The names each file defines, to take them out again when the file changes.
    by_path: Files,
    rx: Option<Receiver<Vec<(String, DefEntry)>>>,
    /// What the index was built for (the project, its list of files and the modules that were on).
    built_for: String,
    /// A build that replaces an index already there: it is gathered here and takes over when it is complete, so that
    /// the old one answers meanwhile.
    next: Option<(Names, Files)>,
    /// Files saved while a build runs: what they define now, which the build (reading older text) must not undo.
    fresh: HashMap<PathBuf, Vec<(String, DefEntry)>>,
    /// The names sorted by their lower case spelling, for completion (made again after a change).
    sorted: std::cell::RefCell<Option<Vec<(String, String)>>>,
    /// The project's list of files is still being read: the index cannot be built yet.
    pub waiting: bool,
}

fn insert_all(map: &mut Names, by_path: &mut Files, entries: Vec<(String, DefEntry)>) {
    for (name, e) in entries {
        let names = by_path.entry(e.path.clone()).or_default();
        if !names.contains(&name) {
            names.push(name.clone());
        }
        map.entry(name).or_default().push(e);
    }
}

fn remove_path(map: &mut Names, by_path: &mut Files, path: &Path) {
    for name in by_path.remove(path).unwrap_or_default() {
        if let Some(v) = map.get_mut(&name) {
            v.retain(|e| e.path != path);
            if v.is_empty() {
                map.remove(&name);
            }
        }
    }
}

impl LangIndex {
    /// Build the index in the background, unless it already is (or is being) built for `key`.
    pub fn ensure(&mut self, key: String, files: Vec<PathBuf>, langs: LanguageSet) {
        self.waiting = false;
        if self.built_for == key {
            return;
        }
        // another project: nothing of the old index is worth showing; the same project read again: it is, until the
        // new one is complete
        let same_project = self.built_for.split('|').next() == key.split('|').next() && !self.map.is_empty();
        self.built_for = key;
        self.fresh.clear();
        self.next = None;
        if same_project {
            self.next = Some(Default::default());
        } else {
            self.map.clear();
            self.by_path.clear();
            *self.sorted.borrow_mut() = None;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let mut batch = vec![];
            for path in files.into_iter().take(40_000) {
                let Some(lang) = indexed_language(&path, &langs) else { continue };
                if std::fs::metadata(&path).map(|m| m.len() > 600_000).unwrap_or(true) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                batch.extend(file_definitions(&path, &text, &lang));
                if batch.len() >= 400 && tx.send(std::mem::take(&mut batch)).is_err() {
                    return;
                }
            }
            let _ = tx.send(batch);
        });
    }

    /// Take what the background build has found so far.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        loop {
            match rx.try_recv() {
                Ok(batch) => {
                    let batch: Vec<(String, DefEntry)> = batch.into_iter().filter(|(_, e)| !self.fresh.contains_key(&e.path)).collect();
                    match self.next.as_mut() {
                        Some((map, by_path)) => insert_all(map, by_path, batch),
                        None => {
                            insert_all(&mut self.map, &mut self.by_path, batch);
                            *self.sorted.borrow_mut() = None;
                        }
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    if let Some((map, by_path)) = self.next.take() {
                        self.map = map;
                        self.by_path = by_path;
                    }
                    // what was saved meanwhile is newer than what the build read
                    for (path, entries) in std::mem::take(&mut self.fresh) {
                        remove_path(&mut self.map, &mut self.by_path, &path);
                        insert_all(&mut self.map, &mut self.by_path, entries);
                    }
                    *self.sorted.borrow_mut() = None;
                    return;
                }
            }
        }
    }

    /// A file was saved (or changed on disk): what it defines now.
    pub fn update_file(&mut self, path: &Path, entries: Vec<(String, DefEntry)>) {
        remove_path(&mut self.map, &mut self.by_path, path);
        if self.rx.is_some() {
            self.fresh.insert(path.to_path_buf(), entries.clone());
        }
        insert_all(&mut self.map, &mut self.by_path, entries);
        *self.sorted.borrow_mut() = None;
    }

    /// The background build is still running, or cannot start before the project's files are known.
    pub fn building(&self) -> bool {
        self.rx.is_some() || self.waiting
    }

    /// The names that start with `prefix` (any case): the name, the kind and the language of its first definition.
    /// The names of `lang` come first, up to `limit` of them, then the others' up to `limit` in all.
    pub fn with_prefix(&self, prefix: &str, limit: usize, lang: Option<&str>) -> Vec<(String, String, String)> {
        let low = prefix.to_lowercase();
        let mut sorted = self.sorted.borrow_mut();
        let sorted = sorted.get_or_insert_with(|| {
            let mut v: Vec<(String, String)> = self.map.iter().filter(|(_, v)| !v.is_empty()).map(|(n, _)| (n.to_lowercase(), n.clone())).collect();
            v.sort();
            v
        });
        let start = sorted.partition_point(|(l, _)| l.as_str() < low.as_str());
        let (mut mine, mut others) = (vec![], vec![]);
        for (_, name) in sorted[start..].iter().take_while(|(l, _)| l.starts_with(&low)) {
            let Some(v) = self.map.get(name) else { continue };
            let e = v.iter().find(|e| Some(e.lang.as_str()) == lang).unwrap_or(&v[0]);
            let item = (name.clone(), e.kind.clone(), e.lang.clone());
            if Some(e.lang.as_str()) == lang {
                if mine.len() < limit {
                    mine.push(item);
                }
            } else if others.len() < limit {
                others.push(item);
            }
            if mine.len() >= limit {
                break;
            }
        }
        let room = limit.saturating_sub(mine.len());
        mine.extend(others.into_iter().take(room));
        mine
    }

    pub fn get(&self, name: &str) -> &[DefEntry] {
        self.map.get(name).map_or(&[], Vec::as_slice)
    }
}

#[derive(Default)]
pub struct HoverState {
    /// The name the pointer rests on: (document id, start, end, revision).
    key: Option<(u64, usize, usize, u64)>,
    since: f64,
    pos: Pos2,
    pub info: Option<Arc<HoverInfo>>,
    asked: bool,
    /// The cache key of the question the REPL is working on.
    waiting: Option<String>,
    rect: Option<Rect>,
    cache: HashMap<String, (f64, Option<Arc<HoverInfo>>)>,
    pub index: LangIndex,
    /// The text search for a definition the index does not know: the name, and where its answer comes.
    pub def_search: Option<(String, Receiver<Vec<LangDefHit>>)>,
}

fn kind_name(code: &str) -> &'static str {
    match code.trim() {
        "o" => "special operator",
        "m" => "macro",
        "f" => "function",
        "c" => "constant",
        "v" => "variable",
        "C" => "class",
        _ => "symbol",
    }
}

impl App {
    pub fn hide_hover(&mut self) {
        self.hover.key = None;
        self.hover.info = None;
        self.hover.asked = false;
        self.hover.waiting = None;
        self.hover.rect = None;
    }

    /// Called after the editor of tab `idx` was drawn: where the pointer rests decides what the popup is about.
    pub fn update_hover(&mut self, ctx: &egui::Context, idx: usize, out: &ViewOut) {
        let busy = self.fake_hover.is_none() && ctx.input(|i| i.pointer.any_down() || i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. } | egui::Event::Text(_) | egui::Event::MouseWheel { .. })));
        if busy || self.popup.is_open() || self.completion.active {
            self.hide_hover();
            return;
        }
        // the pointer on the popup keeps it
        if let Some(r) = self.hover.rect
            && ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.expand(8.0).contains(p))
        {
            return;
        }
        let Some(d) = self.docs.get(idx) else { return };
        let Some(off) = out.hover_char.or(if self.active == idx { self.fake_hover } else { None }).filter(|_| d.diff.is_none()) else {
            self.hide_hover();
            return;
        };
        let lisp = d.ed.is_lisp();
        let lang_ok = lisp || d.ed.language().is_some_and(|l| !l.is_plain() && crate::preview::markup_of(l).is_none());
        if !lang_ok {
            self.hide_hover();
            return;
        }
        let (s, e) = if lisp { d.ed.symbol_range_at(off) } else { d.ed.identifier_range_at(off) };
        let word = d.ed.text().get(s..e).unwrap_or("");
        let skip = word.is_empty()
            || off < s
            || off >= e
            || (!lisp && word.chars().next().is_some_and(|c| c.is_ascii_digit()))
            || (lisp && (word.starts_with(':') || word.trim_matches(|c: char| !c.is_alphanumeric()).is_empty()))
            || matches!(d.view.class_at(off), Some(Class::Comment | Class::String | Class::Number | Class::Char));
        if skip {
            self.hide_hover();
            return;
        }
        let key = (d.id, s, e, d.ed.revision());
        let name = word.to_string();
        if self.hover.key != Some(key) {
            self.hide_hover();
            self.hover.key = Some(key);
            self.hover.since = self.now;
            self.hover.pos = out.hover_pos.or(out.text_rect.map(|r| r.center())).unwrap_or(Pos2::ZERO);
        }
        if self.hover.info.is_none() && !self.hover.asked {
            if self.now - self.hover.since >= DELAY {
                self.hover.asked = true;
                self.resolve_hover(idx, &name, e);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(80));
            }
        }
    }

    fn hover_cache_key(&self, idx: usize, name: &str) -> String {
        let lang = self.docs[idx].ed.language().map(|l| l.id().to_string()).unwrap_or_else(|| "lisp".into());
        format!("{lang}:{name}")
    }

    fn resolve_hover(&mut self, idx: usize, name: &str, end: usize) {
        let ck = self.hover_cache_key(idx, name);
        if let Some((at, info)) = self.hover.cache.get(&ck)
            && self.now - *at < CACHE_SECS
        {
            self.hover.info = info.clone();
            return;
        }
        let lisp = self.docs[idx].ed.is_lisp();
        let found = if lisp {
            self.lisp_hover(name, &ck)
        } else if self.lsp_hover(idx, name, end, &ck) {
            // the language server answers in a moment
            self.hover.waiting = Some(ck.clone());
            None
        } else {
            self.language_hover(idx, name, end)
        };
        match found {
            Some(info) => {
                let info = Arc::new(info);
                if lisp && !info.fallback {
                    self.hover.cache.insert(ck, (self.now, Some(info.clone())));
                }
                self.hover.info = Some(info);
            }
            // the project is still being read: ask again in a moment
            None if !lisp && self.hover.waiting.is_none() && self.hover.index.building() => self.hover.asked = false,
            None => {}
        }
    }

    /// A Lisp name: the project's own definition, else (in the next frames) the running image's answer.
    fn lisp_hover(&mut self, name: &str, ck: &str) -> Option<HoverInfo> {
        self.reindex_open_docs();
        if let Some(p) = self.local_definition_parts(name) {
            let root = self.project_root().map(Path::to_path_buf);
            let source = match &root {
                Some(r) => format!("{}:{}", ide_core::workspace::relative(r, &p.path), p.line + 1),
                None => format!("{}:{}", p.path.display(), p.line + 1),
            };
            return Some(HoverInfo { title: name.to_string(), kind: p.kind, signature: p.signature, doc: p.doc.unwrap_or_default(), source, fallback: false });
        }
        // a function the project defines in C or assembly
        if let Some(hit) = self.project_definitions(name, None, None, None, false).into_iter().next() {
            return Some(HoverInfo { title: name.to_string(), kind: hit.kind, signature: hit.doc.signature, doc: hit.doc.doc, source: hit.source, fallback: false });
        }
        if self.repl_request("hover", name, Pending::Hover { cache_key: ck.to_string(), name: name.to_string() }) {
            self.hover.waiting = Some(ck.to_string());
            return None;
        }
        // no REPL: say what the standard names are, and where the rest of the story is
        let bare = lisp_syntax::symbols::strip_package(name);
        let kind = if lisp_syntax::symbols::is_keyword(bare) {
            "special form / macro"
        } else if lisp_syntax::symbols::is_builtin_function(bare) {
            "function"
        } else {
            return None;
        };
        Some(HoverInfo { title: bare.to_string(), kind: kind.into(), signature: String::new(), doc: crate::i18n::tr("Common Lisp standard name. Start the REPL (Alt+8) to see its arguments and documentation.").to_string(), source: String::new(), fallback: true })
    }

    /// The REPL answered a hover question: kind, arguments and documentation, one per line.
    pub fn hover_reply(&mut self, cache_key: &str, name: &str, payload: &str) {
        let info = if payload.trim().is_empty() {
            None
        } else {
            let mut lines = payload.splitn(3, '\n');
            let kind = kind_name(lines.next().unwrap_or(""));
            let signature = lines.next().unwrap_or("").trim().to_string();
            let doc = lines.next().unwrap_or("").trim().to_string();
            Some(Arc::new(HoverInfo { title: name.to_string(), kind: kind.into(), signature, doc, source: crate::i18n::tr("Common Lisp image").to_string(), fallback: false }))
        };
        self.hover.cache.insert(cache_key.to_string(), (self.now, info.clone()));
        if self.hover.waiting.as_deref() == Some(cache_key) {
            self.hover.waiting = None;
            self.hover.info = info;
        }
    }

    /// The definitions of `name` in the whole project, whatever the language: a C function may be an assembly label,
    /// a Lisp file may call what C defines. Found in the open files (as they are now) and in the background index of
    /// the rest; `lang_first` puts the definitions in that language first and `near` those beside that file. A
    /// definition that takes another number of arguments than the call (`args`, if known) is a different function with
    /// the same name and is left out. `with_lisp` adds the Lisp definitions of the project.
    pub fn project_definitions(&mut self, name: &str, args: Option<usize>, near: Option<PathBuf>, lang_first: Option<&str>, with_lisp: bool) -> Vec<LangDefHit> {
        struct Raw {
            name: String,
            path: PathBuf,
            line: usize,
            kind: String,
            lang: String,
            text: Option<String>,
        }
        let variants = name_variants(name);
        let mut raw: Vec<Raw> = vec![];
        let mut open_paths: Vec<PathBuf> = vec![];
        // the open files, the one being edited first
        let order: Vec<usize> = std::iter::once(self.active).chain((0..self.docs.len()).filter(|i| *i != self.active)).collect();
        for i in order {
            let Some(d) = self.docs.get_mut(i) else { continue };
            let Some(lang) = d.ed.language().cloned() else { continue };
            if d.diff.is_some() || lang.is_plain() || crate::preview::markup_of(&lang).is_some() {
                continue;
            }
            let path = d.path.clone().unwrap_or_default();
            open_paths.push(path.clone());
            let text = d.ed.text().to_string();
            for def in d.defs().iter().filter(|def| variants.contains(&def.name)) {
                raw.push(Raw { name: def.name.clone(), path: path.clone(), line: def.line, kind: def.kind.label().to_string(), lang: lang.id().to_string(), text: Some(text.clone()) });
            }
        }
        // the rest of the project, from the background index
        self.ensure_lang_index();
        let mut cands: Vec<(String, DefEntry)> = variants.iter().flat_map(|v| self.hover.index.get(v).iter().map(|e| (v.clone(), e.clone())).collect::<Vec<_>>()).filter(|(_, e)| !open_paths.contains(&e.path)).collect();
        cands.sort_by_key(|(_, e)| (Some(e.lang.as_str()) != lang_first, near.as_deref().and_then(|n| n.parent()) != e.path.parent(), e.path.clone(), e.line));
        raw.extend(cands.into_iter().take(30).map(|(n, e)| Raw { name: n, path: e.path, line: e.line, kind: e.kind, lang: e.lang, text: None }));
        let root = self.project_root().map(Path::to_path_buf);
        let rel = |path: &Path, line: usize| match &root {
            Some(r) if path.starts_with(r) => format!("{}:{}", ide_core::workspace::relative(r, path), line + 1),
            _ => format!("{}:{}", path.display(), line + 1),
        };
        // (hit, whether the number of arguments of the call fits it)
        let mut found: Vec<(LangDefHit, bool)> = vec![];
        for r in raw {
            let Some(lang) = self.mods.langs.by_id(&r.lang) else { continue };
            let Some(text) = r.text.or_else(|| std::fs::read_to_string(&r.path).ok()) else { continue };
            let doc = docinfo::definition_doc(&text, r.line, &lang);
            // a class, a struct or a type is called with what its constructor takes, not with what its header lists
            // (`class Shape(Base):` is not a function of one argument)
            let callable = matches!(r.kind.as_str(), "function" | "method" | "macro");
            let fits = !callable || docinfo::arity_fits(args, docinfo::signature_arity(&doc.signature, &r.name));
            // where the name is on its line
            let line_start: usize = text.split_inclusive('\n').take(r.line).map(str::len).sum();
            let col = text[line_start.min(text.len())..].lines().next().and_then(|l| l.find(&r.name)).unwrap_or(0);
            let source = rel(&r.path, r.line);
            found.push((LangDefHit { name: r.name, path: r.path, kind: r.kind, offset: line_start + col, doc, source }, fits));
        }
        // the definitions the call fits; when none does, the count was a poor guess and all of them are shown
        let any_fits = found.iter().any(|(_, f)| *f);
        let mut out: Vec<LangDefHit> = found.into_iter().filter(|(_, f)| *f || !any_fits).map(|(h, _)| h).take(12).collect();
        // what the project defines in Lisp
        if with_lisp {
            self.reindex_open_docs();
            for v in &variants {
                if let Some(p) = self.local_definition_parts(v) {
                    let source = rel(&p.path, p.line);
                    out.push(LangDefHit { name: v.clone(), path: p.path, kind: p.kind, offset: p.name_start, doc: docinfo::DocInfo { signature: p.signature, doc: p.doc.unwrap_or_default() }, source });
                }
            }
        }
        out
    }

    /// Start reading the project's definitions in the languages of the modules that are on, unless that is done. It
    /// waits for the project's list of files (an index built before it would stay empty), and is read again whenever
    /// the list is (a refresh, files added or removed, other folders hidden).
    pub fn ensure_lang_index(&mut self) {
        if let Some(p) = self.project.as_ref() {
            if p.files_gen == 0 {
                self.hover.index.waiting = true;
            } else {
                let modules = self.mods.installed.iter().filter(|m| m.enabled).map(|m| format!("{}@{}", m.manifest.id, m.manifest.version)).collect::<Vec<_>>().join(",");
                let key = format!("{}|{}|{}", p.root.display(), p.files_gen, modules);
                let (files, langs) = (p.files.clone(), self.mods.langs.clone());
                self.hover.index.ensure(key, files, langs);
            }
        }
        self.hover.index.poll();
    }

    /// A file of the project was saved: the index learns what it defines now (a function just written is found
    /// before the next time the project is read).
    pub fn lang_index_saved(&mut self, path: &Path, text: &str) {
        let Some(lang) = indexed_language(path, &self.mods.langs) else { return };
        let entries = file_definitions(path, text, &lang);
        self.hover.index.update_file(path, entries);
    }

    /// The last resort of Go to Declaration: read every file of the project for the shapes definitions have (see
    /// [`ide_core::defsearch`]), in the background; [`App::poll_definition_search`] goes there when it is done.
    pub fn search_definition_text(&mut self, name: &str) {
        let Some(finder) = ide_core::defsearch::DefFinder::new(name) else {
            self.toast(crate::i18n::fmt("Cannot find declaration of '{}'", &[&name]), crate::editor_view::Sev::Weak);
            return;
        };
        let Some(p) = self.project.as_ref() else {
            self.toast(crate::i18n::fmt("Cannot find declaration of '{}'", &[&name]), crate::editor_view::Sev::Weak);
            return;
        };
        let (files, root, langs) = (p.files.clone(), p.root.clone(), self.mods.langs.clone());
        let near = self.doc().and_then(|d| d.path.clone());
        // the open files as they are in the editor
        let open: Vec<(PathBuf, String)> = self.docs.iter().filter(|d| d.diff.is_none()).filter_map(|d| d.path.clone().map(|p| (p, d.ed.text().to_string()))).collect();
        let (tx, rx) = mpsc::channel();
        self.hover.def_search = Some((name.to_string(), rx));
        self.status = crate::i18n::fmt("Searching the project for the declaration of '{}'...", &[&name]);
        let name = name.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(definitions_in_text(&finder, &name, files, &open, &root, &langs, near.as_deref()));
        });
    }

    /// The text search of [`App::search_definition_text`] finished: go to what it found.
    pub fn poll_definition_search(&mut self, ctx: &egui::Context) {
        let Some((name, rx)) = &self.hover.def_search else { return };
        match rx.try_recv() {
            Ok(hits) => {
                let name = name.clone();
                self.hover.def_search = None;
                self.status.clear();
                if hits.is_empty() {
                    self.toast(crate::i18n::fmt("Cannot find declaration of '{}'", &[&name]), crate::editor_view::Sev::Weak);
                } else {
                    self.show_hits(hits);
                }
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(60)),
            Err(TryRecvError::Disconnected) => self.hover.def_search = None,
        }
    }

    /// A language server answered what the popup asked about (`key`), or `None` when nobody knows.
    pub fn hover_answer(&mut self, key: &str, info: Option<HoverInfo>) {
        if self.hover.waiting.as_deref() == Some(key) {
            self.hover.waiting = None;
            self.hover.info = info.map(Arc::new);
        }
    }

    /// The hover popup for a name of a language of a module: what the first definition that fits the call says.
    pub(crate) fn language_hover(&mut self, idx: usize, name: &str, end: usize) -> Option<HoverInfo> {
        let args = docinfo::call_arity(self.docs[idx].ed.text(), end);
        let lang = self.docs[idx].ed.language().map(|l| l.id().to_string());
        let near = self.docs[idx].path.clone();
        let hit = self.project_definitions(name, args, near, lang.as_deref(), true).into_iter().next()?;
        Some(HoverInfo { title: name.to_string(), kind: hit.kind, signature: hit.doc.signature, doc: hit.doc.doc, source: hit.source, fallback: false })
    }

    /// Go to the definition of the name at the caret of a document in a language of a module: where its language
    /// server says (it may also know what an `#include` names), else where the project's text has it.
    pub fn goto_language_declaration(&mut self, name: Option<String>) {
        if name.is_none()
            && let Some(d) = self.docs.get(self.active)
        {
            let (s, e) = d.ed.identifier_range_at(d.ed.caret());
            let word = d.ed.text().get(s..e).unwrap_or("").to_string();
            if self.lsp_goto_declaration(&word) {
                return;
            }
        }
        self.goto_language_declaration_text(name);
    }

    /// Go to the definition of the name at the caret (or of `name`) as the project's text has it.
    pub fn goto_language_declaration_text(&mut self, name: Option<String>) {
        let idx = self.active;
        let Some(d) = self.docs.get(idx) else { return };
        let (s, e) = d.ed.identifier_range_at(d.ed.caret());
        let name = name.or_else(|| d.ed.text().get(s..e).filter(|w| !w.is_empty()).map(str::to_string));
        let Some(name) = name else { return };
        let args = docinfo::call_arity(d.ed.text(), e);
        let lang = d.ed.language().map(|l| l.id().to_string());
        let near = d.path.clone();
        let hits = self.project_definitions(&name, args, near, lang.as_deref(), true);
        self.goto_hits(&name, hits);
    }

    /// Go to the one definition found, or let the user choose between several. When the index knows none, the whole
    /// project is read for it (the outline of a language may not know every shape of definition).
    pub fn goto_hits(&mut self, name: &str, hits: Vec<LangDefHit>) {
        if hits.is_empty() {
            if self.project.as_ref().is_some_and(|p| p.files_gen == 0) {
                self.toast(crate::i18n::fmt("Cannot find declaration of '{}' yet: the project is still being read", &[&name]), crate::editor_view::Sev::Weak);
            } else {
                self.search_definition_text(name);
            }
            return;
        }
        self.show_hits(hits);
    }

    /// Go to the one definition, or let the user choose between several.
    fn show_hits(&mut self, hits: Vec<LangDefHit>) {
        match hits.len() {
            0 => {}
            1 => {
                let h = &hits[0];
                self.goto(&h.path, h.offset, true);
            }
            _ => {
                use crate::state::{Item, PTarget, PaletteMode, PaletteState, Popup};
                let items: Vec<Item> = hits
                    .iter()
                    .map(|h| Item {
                        label: h.name.clone(),
                        detail: h.source.clone(),
                        badge: h.kind.chars().next().map(|c| c.to_string()).unwrap_or_default(),
                        section: "",
                        target: PTarget::Symbol { path: h.path.clone(), offset: h.offset },
                        hits: vec![],
                        shortcut: String::new(),
                    })
                    .collect();
                self.popup = Popup::Palette(PaletteState { mode: PaletteMode::Choose, query: String::new(), items: items.clone(), selected: 0, built_for: None, fixed: Some(items), just_opened: true });
            }
        }
    }

    /// Find the uses of `name` in every code file of the project - Lisp, C, assembly, Python... - not in comments and
    /// strings, under the spellings it may have in the other languages (`name`, `_name`, `na-me`).
    pub fn find_usages_everywhere(&mut self, name: &str) {
        if self.project.is_none() || name.is_empty() {
            return;
        }
        self.reindex_open_docs();
        let files = self.project.as_ref().map(|p| p.files.clone()).unwrap_or_default();
        let langs = self.mods.langs.clone();
        let open: Vec<(PathBuf, String)> = self.docs.iter().filter(|d| d.diff.is_none()).filter_map(|d| d.path.clone().map(|p| (p, d.ed.text().to_string()))).collect();
        let title = crate::i18n::fmt("Usages of '{}'", &[&name]);
        self.find_results = crate::state::FindResults { title: crate::i18n::fmt("{} - searching...", &[&title]), hits: vec![] };
        let (tx, rx) = mpsc::channel();
        self.find_rx = Some(rx);
        let variants = name_variants(name);
        std::thread::spawn(move || {
            let mut hits = vec![];
            for path in files {
                let lisp = lisp_syntax::dialect::is_lisp_file(&path);
                let lang = if lisp { None } else { langs.find(&path, None) };
                if !lisp && lang.as_ref().is_none_or(|l| l.is_plain() || crate::preview::markup_of(l).is_some()) {
                    continue;
                }
                // an open file is read as it is in the editor
                let text = match open.iter().find(|(p, _)| *p == path) {
                    Some((_, t)) => t.clone(),
                    None => match std::fs::metadata(&path) {
                        Ok(m) if m.len() <= 2_000_000 => match std::fs::read_to_string(&path) {
                            Ok(t) => t,
                            Err(_) => continue,
                        },
                        _ => continue,
                    },
                };
                match &lang {
                    Some(l) => {
                        let spans = l.highlight(&text);
                        hits.extend(usages_in(&path, &text, &variants, &spans));
                    }
                    None => {
                        // Lisp: by its reader, so that strings and comments are left alone
                        let dialect = lisp_syntax::Dialect::from_path(&path).unwrap_or_default();
                        let mut ranges: Vec<(usize, usize)> = variants.iter().flat_map(|v| ide_core::index::symbol_ranges(&text, dialect, v)).collect();
                        ranges.sort();
                        ranges.dedup();
                        for (s, e) in ranges {
                            let line_start = text[..s].rfind('\n').map_or(0, |i| i + 1);
                            let line_end = text[s..].find('\n').map_or(text.len(), |i| s + i);
                            hits.push(ide_core::search::FileHit { path: path.clone(), line: text[..s].matches('\n').count(), col: text[line_start..s].chars().count(), len: text[s..e].chars().count(), line_text: text[line_start..line_end].trim_end().to_string() });
                        }
                    }
                }
                if hits.len() >= 2000 {
                    break;
                }
            }
            hits.truncate(2000);
            let _ = tx.send((title, hits));
        });
        self.bottom_tool = Some(crate::state::BottomTool::Find);
    }

    /// Find Usages of the name at the caret of a document in a language of a module.
    pub fn find_language_usages(&mut self) {
        let Some(d) = self.docs.get(self.active) else { return };
        let (s, e) = d.ed.identifier_range_at(d.ed.caret());
        let Some(name) = d.ed.text().get(s..e).filter(|w| !w.is_empty()).map(str::to_string) else { return };
        if self.lsp_find_usages(&name) {
            return;
        }
        self.find_usages_everywhere(&name);
    }

    /// The popup itself, near the pointer.
    pub fn draw_hover(&mut self, ctx: &egui::Context) {
        if self.hover.index_poll_needed() {
            self.hover.index.poll();
        }
        self.poll_definition_search(ctx);
        let Some(info) = self.hover.info.clone() else {
            self.hover.rect = None;
            return;
        };
        let pal = self.pal;
        let screen = ctx.content_rect();
        let width = 560.0_f32.min(screen.width() - 24.0);
        let pos = pos2((self.hover.pos.x + 10.0).min(screen.max.x - width - 12.0).max(screen.min.x + 8.0), self.hover.pos.y + 22.0);
        let resp = egui::Area::new(egui::Id::unique("hover_doc")).order(egui::Order::Tooltip).fixed_pos(pos).constrain(true).fade_in(false).show(ctx, |ui| {
            // opaque: it lies over code
            egui::Frame::new().fill(pal.popup_bg.to_opaque()).corner_radius(egui::CornerRadius::same(12)).stroke(egui::Stroke::new(1.0, pal.border)).inner_margin(Margin::symmetric(14, 11)).show(ui, |ui| {
                ui.set_max_width(width - 30.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 6.0);
                // what it is
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(RichText::new(&info.title).font(fonts::semibold(14.0)).color(pal.text));
                    if !info.kind.is_empty() {
                        let g = ui.painter().layout_no_wrap(info.kind.clone(), fonts::ui(11.0), pal.accent);
                        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 18.0), Sense::hover());
                        ui.painter().rect_filled(r, egui::CornerRadius::same(5), pal.accent.gamma_multiply(0.16));
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, &info.kind, fonts::ui(11.0), pal.accent);
                    }
                });
                // its arguments
                if !info.signature.is_empty() {
                    egui::Frame::new().fill(pal.input_bg).corner_radius(egui::CornerRadius::same(6)).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(signature_job(&info.signature, &info.title, &pal)).wrap_mode(egui::TextWrapMode::Wrap));
                    });
                }
                // what its author wrote
                if !info.doc.is_empty() {
                    let mut shown = 0;
                    for para in info.doc.split("\n\n") {
                        if shown >= 14 {
                            ui.label(RichText::new("...").color(pal.text_dim));
                            break;
                        }
                        shown += para.lines().count();
                        ui.add(egui::Label::new(RichText::new(para.trim()).font(FontId::proportional(13.0)).color(pal.text)).wrap_mode(egui::TextWrapMode::Wrap));
                    }
                }
                if !info.source.is_empty() {
                    ui.label(RichText::new(&info.source).font(fonts::ui(11.0)).color(pal.text_dim));
                }
            });
        });
        self.hover.rect = Some(resp.response.rect);
    }
}

impl HoverState {
    fn index_poll_needed(&self) -> bool {
        self.index.rx.is_some()
    }
}

/// The signature in the code font, with the name picked out.
fn signature_job(sig: &str, name: &str, pal: &crate::theme::Palette) -> egui::text::LayoutJob {
    let font = FontId::monospace(12.5);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let plain = |c: Color32| egui::TextFormat { line_height: Some(19.0), ..egui::TextFormat::simple(font.clone(), c) };
    match sig.find(name) {
        Some(p) => {
            job.append(&sig[..p], 0.0, plain(pal.plain));
            job.append(name, 0.0, plain(pal.definition));
            job.append(&sig[p + name.len()..], 0.0, plain(pal.plain));
        }
        None => job.append(sig, 0.0, plain(pal.plain)),
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repl_kinds_have_names() {
        assert_eq!(kind_name("f\n"), "function");
        assert_eq!(kind_name("m"), "macro");
        assert_eq!(kind_name("C"), "class");
        assert_eq!(kind_name("?"), "symbol");
    }

    #[test]
    fn the_index_finds_definitions_of_the_project() {
        let dir = std::env::temp_dir().join(format!("light-hover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("m.c");
        std::fs::write(&file, "// Adds.\nint add(int a, int b) {\n  return a + b;\n}\n").unwrap();
        let mut langs = LanguageSet::new();
        langs
            .add(light_lang::LangDef {
                id: "c".into(),
                name: "C".into(),
                extensions: vec!["c".into()],
                line_comment: "//".into(),
                outline: vec![light_lang::OutlineDef { pattern: r"^[A-Za-z_][\w \t\*]*?[ \t\*]([A-Za-z_]\w*)\s*\([^;{)]*\)\s*\{".into(), kind: "function".into(), name: 1, level: 0, level_mode: "length".into() }],
                ..Default::default()
            })
            .unwrap();
        let mut index = LangIndex::default();
        index.ensure("k".into(), vec![file.clone()], langs);
        for _ in 0..100 {
            index.poll();
            if !index.get("add").is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let hits = index.get("add");
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].line, hits[0].kind.as_str()), (1, "function"));
        assert!(index.get("sub").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The languages of the registry's modules, as the app has them when the modules are on.
    fn registry_langs(modules: &[&str]) -> LanguageSet {
        let mut langs = LanguageSet::new();
        for m in modules {
            let json = std::fs::read_to_string(format!("{}/../../registry/modules/{m}/module.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
            for l in ide_core::modules::Manifest::parse(&json).unwrap().languages {
                langs.add(l).unwrap();
            }
        }
        langs
    }

    fn wait_built(index: &mut LangIndex) {
        for _ in 0..300 {
            index.poll();
            if !index.building() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the index was not built");
    }

    fn temp_project(tag: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("light-defs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in files {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir
    }

    #[test]
    fn saved_files_and_rebuilds_keep_the_index_current() {
        let dir = temp_project("index", &[("a.py", "def alpha():\n    pass\n"), ("b.py", "class Beta:\n    pass\n")]);
        let langs = registry_langs(&["python"]);
        let files = vec![dir.join("a.py"), dir.join("b.py")];
        let mut index = LangIndex::default();
        index.ensure(format!("{}|1|python", dir.display()), files.clone(), langs.clone());
        wait_built(&mut index);
        assert_eq!(index.get("alpha").len(), 1);
        // a save replaces what the file defined
        let lang = langs.by_id("python").unwrap();
        let a = dir.join("a.py");
        index.update_file(&a, file_definitions(&a, "def alpha2():\n    pass\n", &lang));
        assert!(index.get("alpha").is_empty() && index.get("alpha2").len() == 1);
        // the project read again (a new list of files): the old index answers until the new one is complete
        std::fs::write(dir.join("c.py"), "def gamma(): pass\n").unwrap();
        let mut more = files.clone();
        more.push(dir.join("c.py"));
        index.ensure(format!("{}|2|python", dir.display()), more, langs.clone());
        assert_eq!(index.get("Beta").len(), 1, "still there while the new index is read");
        // and a file saved meanwhile is not undone by the build, which read it from the disk before
        index.update_file(&a, file_definitions(&a, "def alpha3(): pass\n", &lang));
        wait_built(&mut index);
        assert_eq!(index.get("gamma").len(), 1);
        assert_eq!(index.get("alpha3").len(), 1);
        assert!(index.get("alpha").is_empty() && index.get("alpha2").is_empty());
        // completion: a prefix in any case, the names of the language asked for first
        let names: Vec<String> = index.with_prefix("AL", 10, Some("python")).into_iter().map(|(n, _, _)| n).collect();
        assert_eq!(names, ["alpha3"]);
        assert_eq!(index.with_prefix("", 2, Some("python")).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_index_waits_for_the_files_of_the_project() {
        let dir = temp_project("wait", &[("m.py", "def late(): pass\n")]);
        let mut app = App::new(ide_core::settings::Settings::default());
        app.mods.langs = registry_langs(&["python"]);
        app.open_project(&dir);
        // asked before the list of files is read: not built empty for good, but waiting
        app.ensure_lang_index();
        assert!(app.hover.index.building() && app.hover.index.get("late").is_empty());
        for _ in 0..300 {
            app.poll_index();
            if app.project.as_ref().unwrap().files_gen > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        app.ensure_lang_index();
        wait_built(&mut app.hover.index);
        assert_eq!(app.hover.index.get("late").len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_class_is_found_whatever_its_constructor_is_called_with() {
        let dir = temp_project("class", &[("shapes.py", "class Shape(Base):\n    def __init__(self):\n        pass\n\ndef area(w, h):\n    return w * h\n"), ("main.py", "s = Shape()\narea(1)\n")]);
        let mut app = App::new(ide_core::settings::Settings::default());
        app.mods.langs = registry_langs(&["python"]);
        app.open_project(&dir);
        for _ in 0..300 {
            app.poll_index();
            app.ensure_lang_index();
            if !app.hover.index.building() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // `Shape()` passes nothing, its header lists a base class: it is still the definition
        let hits = app.project_definitions("Shape", Some(0), None, Some("python"), false);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].source.ends_with("shapes.py:1"));
        // a call with a count no definition takes still goes to the function, the only one there is
        assert_eq!(app.project_definitions("area", Some(1), None, Some("python"), false).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_text_search_finds_what_outlines_miss() {
        let dir = temp_project("text", &[
            ("src/vec.c", "/* Vec2 x; */\ntypedef struct {\n    float x, y;\n} Vec2;\n\nstatic int\nadd(int a, int b)\n{\n    return a + b;\n}\n"),
            ("src/main.c", "int main(void) {\n    Vec2 v;\n    return add(1, 2);\n}\n"),
            ("lib/util.go", "package util\n\nfunc Clamp(x int) int { return x }\n"),
            ("README.md", "func Clamp is described here\n"),
        ]);
        let langs = registry_langs(&["c-cpp"]);
        let files = vec![dir.join("src/vec.c"), dir.join("src/main.c"), dir.join("lib/util.go"), dir.join("README.md")];
        let find = |name: &str| {
            let f = ide_core::defsearch::DefFinder::new(name).unwrap();
            definitions_in_text(&f, name, files.clone(), &[], &dir, &langs, Some(&dir.join("src/main.c")))
        };
        let v = find("Vec2");
        assert_eq!(v.len(), 1, "the one in the comment is not a definition: {v:?}");
        assert!(v[0].source.ends_with("src/vec.c:4") && v[0].kind == "struct");
        let a = find("add");
        assert_eq!(a.len(), 1);
        assert!(a[0].source.ends_with("src/vec.c:7"));
        assert_eq!(&std::fs::read_to_string(dir.join("src/vec.c")).unwrap()[a[0].offset..a[0].offset + 3], "add");
        // a language no module knows, and never in the documentation
        let c = find("Clamp");
        assert_eq!(c.len(), 1);
        assert!(c[0].source.ends_with("lib/util.go:3"));
        assert!(find("missing").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}

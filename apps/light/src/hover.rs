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

/// The definitions of the whole project, whatever the language: name -> where they are.
#[derive(Default)]
pub struct LangIndex {
    map: HashMap<String, Vec<DefEntry>>,
    rx: Option<Receiver<Vec<(String, DefEntry)>>>,
    /// What the index was built for (the project and the modules that were on).
    built_for: String,
}

impl LangIndex {
    /// Build the index in the background, unless it already is (or is being) built for `key`.
    pub fn ensure(&mut self, key: String, files: Vec<PathBuf>, langs: LanguageSet) {
        if self.built_for == key {
            return;
        }
        self.built_for = key;
        self.map.clear();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let mut batch = vec![];
            for path in files.into_iter().take(40_000) {
                let Some(lang) = langs.find(&path, None) else { continue };
                if lang.is_plain() || crate::preview::markup_of(&lang).is_some() || lisp_syntax::dialect::is_lisp_file(&path) {
                    continue;
                }
                if std::fs::metadata(&path).map(|m| m.len() > 600_000).unwrap_or(true) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                for d in lang.outline(&text) {
                    batch.push((d.name.clone(), DefEntry { path: path.clone(), line: d.line, kind: d.kind.label().to_string(), lang: lang.id().to_string() }));
                }
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
                    for (name, e) in batch {
                        self.map.entry(name).or_default().push(e);
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    return;
                }
            }
        }
    }

    /// The background build is still running.
    pub fn building(&self) -> bool {
        self.rx.is_some()
    }

    /// The names that start with `prefix` (any case): the name, the kind and the language of its first definition.
    pub fn with_prefix(&self, prefix: &str, limit: usize) -> Vec<(String, String, String)> {
        let low = prefix.to_lowercase();
        let mut out: Vec<(String, String, String)> = self
            .map
            .iter()
            .filter(|(n, v)| !v.is_empty() && n.to_lowercase().starts_with(&low))
            .map(|(n, v)| (n.clone(), v[0].kind.clone(), v[0].lang.clone()))
            .collect();
        out.sort();
        out.truncate(limit);
        out
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
        let found = if lisp { self.lisp_hover(name, &ck) } else { self.language_hover(idx, name, end) };
        match found {
            Some(info) => {
                let info = Arc::new(info);
                if lisp && !info.fallback {
                    self.hover.cache.insert(ck, (self.now, Some(info.clone())));
                }
                self.hover.info = Some(info);
            }
            // the project is still being read: ask again in a moment
            None if !lisp && self.hover.index.building() => self.hover.asked = false,
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
        raw.extend(cands.into_iter().take(12).map(|(n, e)| Raw { name: n, path: e.path, line: e.line, kind: e.kind, lang: e.lang, text: None }));
        let root = self.project_root().map(Path::to_path_buf);
        let rel = |path: &Path, line: usize| match &root {
            Some(r) if path.starts_with(r) => format!("{}:{}", ide_core::workspace::relative(r, path), line + 1),
            _ => format!("{}:{}", path.display(), line + 1),
        };
        let mut out: Vec<LangDefHit> = vec![];
        for r in raw {
            let Some(lang) = self.mods.langs.by_id(&r.lang) else { continue };
            let Some(text) = r.text.or_else(|| std::fs::read_to_string(&r.path).ok()) else { continue };
            let doc = docinfo::definition_doc(&text, r.line, &lang);
            if !docinfo::arity_fits(args, docinfo::signature_arity(&doc.signature, &r.name)) {
                continue;
            }
            // where the name is on its line
            let line_start: usize = text.split_inclusive('\n').take(r.line).map(str::len).sum();
            let col = text[line_start.min(text.len())..].lines().next().and_then(|l| l.find(&r.name)).unwrap_or(0);
            let source = rel(&r.path, r.line);
            out.push(LangDefHit { name: r.name, path: r.path, kind: r.kind, offset: line_start + col, doc, source });
            if out.len() >= 8 {
                break;
            }
        }
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

    /// Start reading the project's definitions in the languages of the modules that are on, unless that is done.
    pub fn ensure_lang_index(&mut self) {
        if let Some(p) = self.project.as_ref() {
            let key = format!("{}|{}", p.root.display(), self.mods.installed.iter().filter(|m| m.enabled).map(|m| format!("{}@{}", m.manifest.id, m.manifest.version)).collect::<Vec<_>>().join(","));
            let (files, langs) = (p.files.clone(), self.mods.langs.clone());
            self.hover.index.ensure(key, files, langs);
        }
        self.hover.index.poll();
    }

    /// The hover popup for a name of a language of a module: what the first definition that fits the call says.
    fn language_hover(&mut self, idx: usize, name: &str, end: usize) -> Option<HoverInfo> {
        let args = docinfo::call_arity(self.docs[idx].ed.text(), end);
        let lang = self.docs[idx].ed.language().map(|l| l.id().to_string());
        let near = self.docs[idx].path.clone();
        let hit = self.project_definitions(name, args, near, lang.as_deref(), true).into_iter().next()?;
        Some(HoverInfo { title: name.to_string(), kind: hit.kind, signature: hit.doc.signature, doc: hit.doc.doc, source: hit.source, fallback: false })
    }

    /// Go to the definition of the name at the caret of a document in a language of a module.
    pub fn goto_language_declaration(&mut self, name: Option<String>) {
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

    /// Go to the one definition found, or let the user choose between several; say so when there is none.
    pub fn goto_hits(&mut self, name: &str, hits: Vec<LangDefHit>) {
        match hits.len() {
            0 => {
                // the project may still be being read
                let msg = if self.hover.index.building() {
                    crate::i18n::fmt("Cannot find declaration of '{}' yet: the project is still being read", &[&name])
                } else {
                    crate::i18n::fmt("Cannot find declaration of '{}'", &[&name])
                };
                self.toast(msg, crate::editor_view::Sev::Weak);
            }
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
        self.find_usages_everywhere(&name);
    }

    /// The popup itself, near the pointer.
    pub fn draw_hover(&mut self, ctx: &egui::Context) {
        if self.hover.index_poll_needed() {
            self.hover.index.poll();
        }
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
            egui::Frame::new().fill(pal.popup_bg).corner_radius(egui::CornerRadius::same(12)).stroke(egui::Stroke::new(1.0, pal.border)).inner_margin(Margin::symmetric(14, 11)).show(ui, |ui| {
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
}

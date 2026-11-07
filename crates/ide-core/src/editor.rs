//! The text editing model: buffer, caret/selection, undo history and all editing commands.
//! It is completely UI independent so every behaviour can be unit tested.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use light_lang::{IndentMode, Language};
use lisp_syntax::highlight::Span;

use lisp_syntax::indent;
use lisp_syntax::lexer::TokenKind;
use lisp_syntax::structure::{self, Rewrite};
use lisp_syntax::{Dialect, Parsed, parse};

pub const INDENT_WIDTH: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    removed: String,
    inserted: String,
}

#[derive(Clone, Debug)]
struct Entry {
    edits: Vec<Edit>,
    before: (usize, usize),
    after: (usize, usize),
    kind: EditKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructuralOp {
    SlurpForward,
    BarfForward,
    SlurpBackward,
    BarfBackward,
    Raise,
    Splice,
    WrapRound,
    KillSexp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SexpMove {
    Forward,
    Backward,
    Up,
    Down,
}

/// Character classes for word movement.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Word,
    Space,
    Other,
}

/// A rule of a makefile (`all: main.o`, `%.o: %.c`), not a variable (`X := 1`, `Y = a:b`) nor a recipe or a comment.
fn is_make_rule(line: &str) -> bool {
    if line.starts_with(['\t', ' ', '#']) {
        return false;
    }
    let code = line.split('#').next().unwrap_or("");
    match code.find(':') {
        Some(i) => !code[..i].contains('=') && !code[i..].starts_with(":=") && !code[i..].starts_with("::="),
        None => false,
    }
}

fn class_of(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

pub struct Editor {
    text: String,
    caret: usize,
    anchor: usize,
    want_col: Option<usize>,
    dialect: Dialect,
    /// The language of a file that is not Lisp (a module's: C, Python, Markdown...); `None` is Lisp. Plain text is a
    /// language too, one without rules.
    other: Option<Arc<Language>>,
    /// Highlighting spans of `other`, for the revision they were made for.
    lang_cache: RefCell<Option<(u64, Rc<Vec<Span>>)>>,
    problem_cache: RefCell<Option<(u64, Rc<Vec<light_lang::Problem>>)>>,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    pending: Vec<Edit>,
    revision: u64,
    saved_revision: u64,
    parsed: RefCell<Option<(u64, Rc<Parsed>)>>,
    lines: RefCell<Option<(u64, Rc<Vec<usize>>)>>,
    sel_stack: Vec<(usize, usize)>,
    pub line_ending: LineEnding,
}

impl Editor {
    pub fn new(text: &str, dialect: Dialect) -> Editor {
        let line_ending = if text.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf };
        Editor {
            text: text.replace("\r\n", "\n"),
            caret: 0,
            anchor: 0,
            want_col: None,
            dialect,
            other: None,
            lang_cache: RefCell::new(None),
            problem_cache: RefCell::new(None),
            undo: Vec::new(),
            redo: Vec::new(),
            pending: Vec::new(),
            revision: 0,
            saved_revision: 0,
            parsed: RefCell::new(None),
            lines: RefCell::new(None),
            sel_stack: Vec::new(),
            line_ending,
        }
    }

    // ---------------------------------------------------------------- accessors

    pub fn text(&self) -> &str {
        &self.text
    }
    /// Text as it should be written to disk (original line endings).
    pub fn text_for_disk(&self) -> String {
        match self.line_ending {
            LineEnding::Lf => self.text.clone(),
            LineEnding::CrLf => self.text.replace('\n', "\r\n"),
        }
    }
    /// Write the file with `le` from now on. The text in memory keeps `\n`, so only the next save changes; the file counts
    /// as modified until then. Returns `false` when it already had `le`.
    pub fn set_line_ending(&mut self, le: LineEnding) -> bool {
        if self.line_ending == le {
            return false;
        }
        self.line_ending = le;
        self.revision += 1;
        true
    }
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }
    pub fn set_dialect(&mut self, d: Dialect) {
        self.dialect = d;
        *self.parsed.borrow_mut() = None;
    }
    /// The language of a file that is not Lisp (`None`: Lisp).
    pub fn language(&self) -> Option<&Arc<Language>> {
        self.other.as_ref()
    }
    pub fn set_language(&mut self, l: Option<Arc<Language>>) {
        self.other = l;
        *self.lang_cache.borrow_mut() = None;
        *self.problem_cache.borrow_mut() = None;
    }
    pub fn is_lisp(&self) -> bool {
        self.other.is_none()
    }
    /// Highlighting spans of the text in its language, made once per revision (`None` for Lisp, which has its own).
    pub fn language_spans(&self) -> Option<Rc<Vec<Span>>> {
        let lang = self.other.as_ref()?;
        if let Some((rev, s)) = &*self.lang_cache.borrow()
            && *rev == self.revision
        {
            return Some(s.clone());
        }
        let spans = Rc::new(lang.highlight(&self.text));
        *self.lang_cache.borrow_mut() = Some((self.revision, spans.clone()));
        Some(spans)
    }
    /// What the language's own checks find in the text (brackets that do not pair up...), cached per revision. Empty
    /// for Lisp, which has its reader's diagnostics.
    pub fn language_problems(&self) -> Rc<Vec<light_lang::Problem>> {
        let Some(lang) = self.other.as_ref() else { return Rc::new(Vec::new()) };
        if let Some((rev, p)) = &*self.problem_cache.borrow()
            && *rev == self.revision
        {
            return p.clone();
        }
        let spans = self.language_spans().unwrap_or_default();
        let found = Rc::new(lang.problems(&self.text, &spans));
        *self.problem_cache.borrow_mut() = Some((self.revision, found.clone()));
        found
    }

    /// The bracket matching the one at the caret: by the Lisp reader, or by the language's own grammar.
    pub fn matching_bracket(&self, off: usize) -> Option<(usize, usize)> {
        match &self.other {
            Some(l) => l.matching_bracket(&self.text, &self.language_spans()?, off),
            None => self.parsed().matching_bracket(off),
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn is_modified(&self) -> bool {
        self.revision != self.saved_revision
    }
    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }
    pub fn caret(&self) -> usize {
        self.caret
    }
    pub fn anchor(&self) -> usize {
        self.anchor
    }
    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }
    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }
    pub fn selected_text(&self) -> &str {
        let (s, e) = self.selection();
        &self.text[s..e]
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn parsed(&self) -> Rc<Parsed> {
        if let Some((rev, p)) = &*self.parsed.borrow()
            && *rev == self.revision
        {
            return p.clone();
        }
        let p = Rc::new(parse(&self.text, self.dialect));
        *self.parsed.borrow_mut() = Some((self.revision, p.clone()));
        p
    }

    // ---------------------------------------------------------------- positions

    /// Start offsets of all lines, cached per revision (all edits bump the revision).
    fn line_index(&self) -> Rc<Vec<usize>> {
        if let Some((rev, li)) = &*self.lines.borrow()
            && *rev == self.revision
        {
            return li.clone();
        }
        let li = Rc::new(lisp_syntax::indent::line_starts(&self.text));
        *self.lines.borrow_mut() = Some((self.revision, li.clone()));
        li
    }

    pub fn line_count(&self) -> usize {
        self.line_index().len()
    }

    pub fn line_start_of(&self, offset: usize) -> usize {
        let li = self.line_index();
        li[li.partition_point(|&s| s <= offset) - 1]
    }

    pub fn line_end_of(&self, offset: usize) -> usize {
        let li = self.line_index();
        let i = li.partition_point(|&s| s <= offset);
        if i < li.len() { li[i] - 1 } else { self.text.len() }
    }

    pub fn line_of(&self, offset: usize) -> usize {
        let li = self.line_index();
        li.partition_point(|&s| s <= offset) - 1
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.line_index().get(line).copied().unwrap_or(self.text.len())
    }

    pub fn line_text(&self, line: usize) -> &str {
        let s = self.line_start(line);
        &self.text[s..self.line_end_of(s)]
    }

    /// (line, column in chars), both 0-based.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let ls = self.line_start_of(offset);
        (self.line_of(offset), self.text[ls..offset].chars().count())
    }

    pub fn offset_of(&self, line: usize, col: usize) -> usize {
        let ls = self.line_start(line);
        let le = self.line_end_of(ls);
        let mut off = ls;
        for (n, (i, c)) in self.text[ls..le].char_indices().enumerate() {
            if n == col {
                return ls + i;
            }
            off = ls + i + c.len_utf8();
        }
        off.min(le)
    }

    fn prev_boundary(&self, off: usize) -> usize {
        self.text[..off].char_indices().next_back().map_or(0, |(i, _)| i)
    }
    fn next_boundary(&self, off: usize) -> usize {
        self.text[off..].chars().next().map_or(off, |c| off + c.len_utf8())
    }
    fn char_before(&self, off: usize) -> Option<char> {
        self.text[..off].chars().next_back()
    }
    fn char_after(&self, off: usize) -> Option<char> {
        self.text[off..].chars().next()
    }

    // ---------------------------------------------------------------- history primitives

    fn splice(&mut self, start: usize, end: usize, ins: &str) {
        if start == end && ins.is_empty() {
            return;
        }
        let removed = self.text[start..end].to_string();
        self.text.replace_range(start..end, ins);
        self.pending.push(Edit { start, removed, inserted: ins.to_string() });
        self.revision += 1;
    }

    fn transact<R>(&mut self, kind: EditKind, f: impl FnOnce(&mut Self) -> R) -> R {
        let before = (self.caret, self.anchor);
        let r = f(self);
        let edits = std::mem::take(&mut self.pending);
        if !edits.is_empty() {
            self.sel_stack.clear();
            let after = (self.caret, self.anchor);
            self.redo.clear();
            let coalesce = kind != EditKind::Other
                && self.undo.last().is_some_and(|l| {
                    l.kind == kind
                        && l.after == before
                        && before.0 == before.1
                        && (kind == EditKind::Deleting || !l.edits.last().is_some_and(|e| e.inserted.ends_with(char::is_whitespace)))
                });
            if coalesce {
                let l = self.undo.last_mut().unwrap();
                l.edits.extend(edits);
                l.after = after;
            } else {
                self.undo.push(Entry { edits, before, after, kind });
            }
        }
        r
    }

    fn set_caret_raw(&mut self, caret: usize, anchor: usize) {
        self.caret = caret.min(self.text.len());
        self.anchor = anchor.min(self.text.len());
    }

    pub fn undo(&mut self) -> bool {
        let Some(e) = self.undo.pop() else { return false };
        for ed in e.edits.iter().rev() {
            let end = ed.start + ed.inserted.len();
            self.text.replace_range(ed.start..end, &ed.removed);
        }
        self.revision += 1;
        self.set_caret_raw(e.before.0, e.before.1);
        self.want_col = None;
        self.redo.push(e);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(e) = self.redo.pop() else { return false };
        for ed in &e.edits {
            let end = ed.start + ed.removed.len();
            self.text.replace_range(ed.start..end, &ed.inserted);
        }
        self.revision += 1;
        self.set_caret_raw(e.after.0, e.after.1);
        self.want_col = None;
        self.undo.push(e);
        true
    }

    /// Replace the whole content (used for reload); clears history.
    pub fn set_text(&mut self, text: &str) {
        self.line_ending = if text.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf };
        self.text = text.replace("\r\n", "\n");
        self.undo.clear();
        self.redo.clear();
        self.pending.clear();
        self.revision += 1;
        self.saved_revision = self.revision;
        let n = self.text.len();
        self.set_caret_raw(self.caret.min(n), self.caret.min(n));
        while !self.text.is_char_boundary(self.caret) {
            self.caret -= 1;
        }
        self.anchor = self.caret;
    }

    // ---------------------------------------------------------------- selection / movement

    pub fn set_caret(&mut self, offset: usize, extend: bool) {
        let offset = offset.min(self.text.len());
        self.caret = offset;
        if !extend {
            self.anchor = offset;
        }
        self.want_col = None;
        self.sel_stack.clear();
    }

    pub fn select_range(&mut self, start: usize, end: usize) {
        self.anchor = start.min(self.text.len());
        self.caret = end.min(self.text.len());
        self.want_col = None;
    }

    pub fn select_all(&mut self) {
        self.select_range(0, self.text.len());
    }

    fn move_to(&mut self, off: usize, extend: bool, keep_want: bool) {
        self.caret = off;
        if !extend {
            self.anchor = off;
        }
        if !keep_want {
            self.want_col = None;
        }
        self.sel_stack.clear();
    }

    pub fn move_left(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            let s = self.selection().0;
            return self.move_to(s, false, false);
        }
        let o = self.prev_boundary(self.caret);
        self.move_to(o, extend, false);
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            let e = self.selection().1;
            return self.move_to(e, false, false);
        }
        let o = self.next_boundary(self.caret);
        self.move_to(o, extend, false);
    }

    pub fn word_left_offset(&self, from: usize) -> usize {
        let mut o = from;
        while let Some(c) = self.char_before(o) {
            if class_of(c) != Class::Space {
                break;
            }
            o -= c.len_utf8();
        }
        if let Some(c) = self.char_before(o) {
            let cls = class_of(c);
            while let Some(c) = self.char_before(o) {
                if class_of(c) != cls {
                    break;
                }
                o -= c.len_utf8();
            }
        }
        o
    }

    pub fn word_right_offset(&self, from: usize) -> usize {
        let mut o = from;
        if let Some(c) = self.char_after(o) {
            let cls = class_of(c);
            if cls != Class::Space {
                while let Some(c) = self.char_after(o) {
                    if class_of(c) != cls {
                        break;
                    }
                    o += c.len_utf8();
                }
            }
        }
        while let Some(c) = self.char_after(o) {
            if class_of(c) != Class::Space || c == '\n' {
                break;
            }
            o += c.len_utf8();
        }
        o
    }

    pub fn move_word_left(&mut self, extend: bool) {
        let o = self.word_left_offset(self.caret);
        self.move_to(o, extend, false);
    }

    pub fn move_word_right(&mut self, extend: bool) {
        let o = self.word_right_offset(self.caret);
        self.move_to(o, extend, false);
    }

    /// Smart home: first non-blank, then column 0.
    pub fn move_line_start(&mut self, extend: bool) {
        let ls = self.line_start_of(self.caret);
        let le = self.line_end_of(ls);
        let first = ls + self.text[ls..le].len() - self.text[ls..le].trim_start().len();
        let target = if self.caret == first { ls } else { first };
        self.move_to(target, extend, false);
    }

    pub fn move_line_end(&mut self, extend: bool) {
        let e = self.line_end_of(self.caret);
        self.move_to(e, extend, false);
    }

    pub fn move_doc_start(&mut self, extend: bool) {
        self.move_to(0, extend, false);
    }

    pub fn move_doc_end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend, false);
    }

    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        let (line, col) = self.line_col(self.caret);
        let want = self.want_col.unwrap_or(col);
        let last = self.line_count() - 1;
        let target = line as isize + delta;
        if target < 0 {
            return self.move_to(0, extend, true);
        }
        if target as usize > last {
            return self.move_to(self.text.len(), extend, true);
        }
        let off = self.offset_of(target as usize, want);
        self.move_to(off, extend, true);
        self.want_col = Some(want);
    }

    pub fn select_word_at(&mut self, offset: usize) {
        let mut s = offset;
        let mut e = offset;
        let cls = self.char_after(offset).map(class_of).or_else(|| self.char_before(offset).map(class_of)).unwrap_or(Class::Other);
        if self.char_after(offset).is_none() {
            s = self.word_left_offset(offset).min(offset);
        } else {
            while let Some(c) = self.char_before(s) {
                if class_of(c) != cls {
                    break;
                }
                s -= c.len_utf8();
            }
            while let Some(c) = self.char_after(e) {
                if class_of(c) != cls {
                    break;
                }
                e += c.len_utf8();
            }
        }
        self.select_range(s, e.max(s));
    }

    pub fn select_line_at(&mut self, offset: usize) {
        let s = self.line_start_of(offset);
        let e = self.line_end_of(offset);
        let e = if e < self.text.len() { e + 1 } else { e };
        self.select_range(s, e);
    }

    /// Word under (or directly before) the caret, for completion prefixes and quick doc.
    pub fn symbol_range_at(&self, offset: usize) -> (usize, usize) {
        let is_sym = |c: char| !(c.is_whitespace() || "()[]{}\"';`,".contains(c));
        let mut s = offset;
        let mut e = offset;
        while let Some(c) = self.char_before(s) {
            if !is_sym(c) {
                break;
            }
            s -= c.len_utf8();
        }
        while let Some(c) = self.char_after(e) {
            if !is_sym(c) {
                break;
            }
            e += c.len_utf8();
        }
        (s, e)
    }

    /// The identifier that contains `offset` or ends at it, including extra characters named by the module.
    pub fn identifier_range_at(&self, offset: usize) -> (usize, usize) {
        let is_id = |c: char| self.other.as_ref().map_or(c.is_alphanumeric() || c == '_', |lang| lang.is_identifier_char(c));
        let mut s = offset;
        let mut e = offset;
        while let Some(c) = self.char_before(s) {
            if !is_id(c) {
                break;
            }
            s -= c.len_utf8();
        }
        while let Some(c) = self.char_after(e) {
            if !is_id(c) {
                break;
            }
            e += c.len_utf8();
        }
        (s, e)
    }

    pub fn extend_selection(&mut self) {
        let (s, e) = self.selection();
        let cur = (s, e);
        // First step on an empty selection: the identifier-ish word.
        let next = if s == e {
            let (ws, we) = self.symbol_range_at(s);
            if ws < we { (ws, we) } else { cur }
        } else {
            cur
        };
        let next = if next == cur {
            structure::extend_selection(&self.text, &self.parsed(), cur)
        } else {
            next
        };
        if next != cur {
            let mut stack = std::mem::take(&mut self.sel_stack);
            stack.push((self.anchor, self.caret));
            self.select_range(next.0, next.1);
            self.sel_stack = stack;
        }
    }

    pub fn shrink_selection(&mut self) {
        if let Some((a, c)) = self.sel_stack.pop() {
            let stack = std::mem::take(&mut self.sel_stack);
            self.select_range(a, c);
            self.sel_stack = stack;
        }
    }

    // ---------------------------------------------------------------- basic editing

    fn delete_selection_inner(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        let (s, e) = self.selection();
        self.splice(s, e, "");
        self.set_caret_raw(s, s);
        true
    }

    /// Replace the selection (or insert at the caret) with `s`, caret after it.
    fn replace_selection_with(&mut self, s: &str) {
        let (a, b) = self.selection();
        self.splice(a, b, s);
        let c = a + s.len();
        self.set_caret_raw(c, c);
    }

    /// The token with `start < off <= end` (the one the caret sits in or right after).
    fn token_before(&self, off: usize) -> Option<lisp_syntax::Token> {
        let p = self.parsed();
        let idx = p.tokens.partition_point(|t| t.end < off);
        p.tokens.get(idx).copied().filter(|t| t.start < off)
    }

    fn in_string(&self, off: usize) -> bool {
        self.token_before(off).is_some_and(|t| match t.kind {
            TokenKind::String => off < t.end,
            TokenKind::UnterminatedString => true,
            _ => false,
        })
    }

    fn in_comment(&self, off: usize) -> bool {
        self.token_before(off).is_some_and(|t| match t.kind {
            TokenKind::Comment | TokenKind::UnterminatedBlockComment => true,
            TokenKind::BlockComment => off < t.end,
            _ => false,
        })
    }

    /// Insert text typed by the user. Single characters get IDE-style auto pairing.
    pub fn type_text(&mut self, s: &str) {
        let mut it = s.chars();
        let single = match (it.next(), it.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        };
        self.want_col = None;
        match single {
            Some(c) => self.transact(EditKind::Typing, |ed| ed.type_char(c)),
            None => self.transact(EditKind::Other, |ed| ed.replace_selection_with(s)),
        }
    }

    fn type_char(&mut self, c: char) {
        let caret = self.caret;
        let sel = self.has_selection();
        let in_str = self.in_string(caret);
        let in_com = self.in_comment(caret);
        let next = self.char_after(self.selection().1);
        let prev = self.char_before(self.selection().0);
        let pairs = [('(', ')'), ('[', ']'), ('{', '}')];
        match c {
            '(' | '[' | '{' if !in_com && !in_str => {
                let close = pairs.iter().find(|p| p.0 == c).unwrap().1;
                if sel {
                    let (s, e) = self.selection();
                    let inner = self.text[s..e].to_string();
                    self.splice(s, e, &format!("{c}{inner}{close}"));
                    self.anchor = s + 1;
                    self.caret = s + 1 + inner.len();
                } else if prev == Some('\\') || next.is_some_and(|n| n.is_alphanumeric() || n == '_') {
                    self.replace_selection_with(&c.to_string());
                } else {
                    self.replace_selection_with(&format!("{c}{close}"));
                    self.caret -= close.len_utf8();
                    self.anchor = self.caret;
                }
            }
            ')' | ']' | '}' if !sel && !in_str && !in_com && next == Some(c) && prev != Some('\\') => {
                let o = self.next_boundary(caret);
                self.set_caret_raw(o, o);
            }
            '}' if !sel && self.other.as_ref().is_some_and(|l| l.braces()) && caret > self.line_start_of(caret) && self.text[self.line_start_of(caret)..caret].chars().all(|ch| ch == ' ' || ch == '\t') => {
                // a closing brace typed first on a line goes back one level
                let ls = self.line_start_of(caret);
                let keep = (caret - ls).saturating_sub(self.other.as_ref().map_or(4, |l| l.indent_size()));
                self.splice(ls + keep, caret, "");
                let c = ls + keep;
                self.set_caret_raw(c, c);
                self.replace_selection_with("}");
            }
            '"' if !in_com => {
                if in_str {
                    if !sel && next == Some('"') && prev != Some('\\') {
                        let o = self.next_boundary(caret);
                        self.set_caret_raw(o, o);
                    } else {
                        self.replace_selection_with("\\\"");
                    }
                } else if sel {
                    let (s, e) = self.selection();
                    let inner = self.text[s..e].to_string();
                    self.splice(s, e, &format!("\"{inner}\""));
                    self.anchor = s + 1;
                    self.caret = s + 1 + inner.len();
                } else if prev == Some('\\') || prev.is_some_and(|p| p.is_alphanumeric()) || next.is_some_and(|n| n.is_alphanumeric()) {
                    self.replace_selection_with("\"");
                } else {
                    self.replace_selection_with("\"\"");
                    self.caret -= 1;
                    self.anchor = self.caret;
                }
            }
            _ => self.replace_selection_with(&c.to_string()),
        }
    }

    pub fn insert_text_raw(&mut self, s: &str) {
        self.want_col = None;
        self.transact(EditKind::Other, |ed| ed.replace_selection_with(s));
    }

    pub fn paste(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n");
        self.insert_text_raw(&s);
    }

    /// Text to put on the clipboard: the selection, or the whole current line.
    pub fn copy_text(&self) -> String {
        if self.has_selection() {
            self.selected_text().to_string()
        } else {
            let s = self.line_start_of(self.caret);
            let e = self.line_end_of(self.caret);
            let e = if e < self.text.len() { e + 1 } else { e };
            let mut t = self.text[s..e].to_string();
            if !t.ends_with('\n') {
                t.push('\n');
            }
            t
        }
    }

    pub fn cut(&mut self) -> String {
        let t = self.copy_text();
        if self.has_selection() {
            self.transact(EditKind::Other, |ed| {
                ed.delete_selection_inner();
            });
        } else {
            self.delete_line();
        }
        t
    }

    pub fn backspace(&mut self) {
        self.want_col = None;
        self.transact(EditKind::Deleting, |ed| {
            if ed.delete_selection_inner() {
                return;
            }
            let c = ed.caret;
            if c == 0 {
                return;
            }
            let prev = ed.prev_boundary(c);
            // delete an empty auto-pair together
            let pair = matches!((ed.char_before(c), ed.char_after(c)), (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}')) | (Some('"'), Some('"')));
            let end = if pair { ed.next_boundary(c) } else { c };
            ed.splice(prev, end, "");
            ed.set_caret_raw(prev, prev);
        });
    }

    pub fn delete_forward(&mut self) {
        self.want_col = None;
        self.transact(EditKind::Deleting, |ed| {
            if ed.delete_selection_inner() {
                return;
            }
            let c = ed.caret;
            let n = ed.next_boundary(c);
            ed.splice(c, n, "");
        });
    }

    pub fn delete_word_back(&mut self) {
        self.transact(EditKind::Other, |ed| {
            if ed.delete_selection_inner() {
                return;
            }
            let s = ed.word_left_offset(ed.caret);
            let c = ed.caret;
            ed.splice(s, c, "");
            ed.set_caret_raw(s, s);
        });
    }

    pub fn delete_word_forward(&mut self) {
        self.transact(EditKind::Other, |ed| {
            if ed.delete_selection_inner() {
                return;
            }
            let e = ed.word_right_offset(ed.caret);
            let c = ed.caret;
            ed.splice(c, e, "");
        });
    }

    pub fn delete_to_line_end(&mut self) {
        self.transact(EditKind::Other, |ed| {
            let c = ed.caret;
            let mut e = ed.line_end_of(c);
            if e == c && e < ed.text.len() {
                e += 1;
            }
            ed.splice(c, e, "");
        });
    }

    /// Enter: newline plus Lisp indentation.
    pub fn insert_newline(&mut self) {
        self.want_col = None;
        self.transact(EditKind::Other, |ed| {
            ed.delete_selection_inner();
            let c = ed.caret;
            // Markdown and reStructuredText: Enter in a list or a quotation carries the marker on (and on an empty item
            // ends the list)
            if let Some(lang) = ed.other.clone()
                && lang.indent_mode() == IndentMode::Markup
            {
                let ls = ed.line_start_of(c);
                let at_end = ed.char_after(c).is_none_or(|ch| ch == '\n');
                // splitting an item in the middle starts a new item; only an empty one, at the end, ends the list
                if let Some(cont) = lang.continue_line(&ed.text[ls..c]).filter(|k| !k.end_list || at_end) {
                    if cont.end_list {
                        ed.splice(ls, c, "");
                        ed.set_caret_raw(ls, ls);
                        ed.splice(ls, ls, "\n");
                        ed.set_caret_raw(ls + 1, ls + 1);
                    } else {
                        let ins = format!("\n{}", cont.insert);
                        ed.splice(c, c, &ins);
                        ed.set_caret_raw(c + ins.len(), c + ins.len());
                    }
                    return;
                }
            }
            let split_pair = ed.other.as_ref().is_some_and(|l| {
                (l.braces() && ed.char_before(c) == Some('{') && ed.char_after(c) == Some('}'))
                    || l.split_tag_pair(&ed.text[ed.line_start_of(c)..c], &ed.text[c..ed.line_end_of(c)])
            });
            if split_pair {
                // `{|}` or `<div>|</div>` + Enter: the closer gets its own line, the caret an indented one.
                ed.splice(c, c, "\n\n");
                ed.indent_line_at(c + 2);
                ed.indent_line_at(c + 1);
                return;
            }
            ed.splice(c, c, "\n");
            ed.indent_line_at(c + 1);
        });
    }

    /// Re-indent the line starting at `ls` and put the caret after the new indent.
    fn indent_line_at(&mut self, ls: usize) {
        if let Some(lang) = self.other.clone() {
            self.indent_line_plain(ls, &lang);
            return;
        }
        let p = Rc::new(parse(&self.text, self.dialect));
        let want = indent::indent_for_line_start_parsed(&self.text, &p, ls).unwrap_or(0);
        let le = self.line_end_of(ls);
        let ws = self.text[ls..le].len() - self.text[ls..le].trim_start_matches([' ', '\t']).len();
        self.splice(ls, ls + ws, &" ".repeat(want));
        let c = ls + want;
        self.set_caret_raw(c, c);
    }

    /// Indentation for a language that is not Lisp, as its definition says (see `Language::indent_for`).
    fn indent_line_plain(&mut self, ls: usize, lang: &Language) {
        let mut prev = String::new();
        let mut p = ls;
        while p > 0 {
            let ps = self.line_start_of(p - 1);
            let line = &self.text[ps..p - 1];
            if !line.trim().is_empty() {
                prev = line.to_string();
                break;
            }
            p = ps;
        }
        let le = self.line_end_of(ls);
        let cur = self.text[ls..le].to_string();
        let ws = cur.len() - cur.trim_start_matches([' ', '\t']).len();
        let mut want = lang.indent_for(&prev, &cur);
        // the line under a rule of a makefile is its recipe: it starts with a tab
        if lang.id() == "make" && want.is_empty() && is_make_rule(&prev) {
            want = "\t".into();
        }
        self.splice(ls, ls + ws, &want);
        let c = ls + want.len();
        self.set_caret_raw(c, c);
    }

    /// Shift+Enter: start a new line below, regardless of the caret position.
    pub fn new_line_below(&mut self) {
        self.transact(EditKind::Other, |ed| {
            let e = ed.line_end_of(ed.caret);
            ed.splice(e, e, "\n");
            ed.indent_line_at(e + 1);
        });
    }

    /// Ctrl+Alt+Enter: start a new line above.
    pub fn new_line_above(&mut self) {
        self.transact(EditKind::Other, |ed| {
            let s = ed.line_start_of(ed.caret);
            ed.splice(s, s, "\n");
            ed.indent_line_at(s);
        });
    }

    // ---------------------------------------------------------------- line commands

    /// Lines (inclusive) touched by the selection; a selection ending at column 0 excludes that last line.
    pub fn selected_lines(&self) -> (usize, usize) {
        let (s, e) = self.selection();
        let l1 = self.line_of(s);
        let mut l2 = self.line_of(e);
        if e > s && self.line_start_of(e) == e && l2 > l1 {
            l2 -= 1;
        }
        (l1, l2)
    }

    fn lines_byte_range(&self, l1: usize, l2: usize) -> (usize, usize) {
        let s = self.line_start(l1);
        let e = self.line_end_of(self.line_start(l2));
        (s, e)
    }

    pub fn delete_line(&mut self) {
        self.want_col = None;
        self.transact(EditKind::Other, |ed| {
            let (l1, l2) = ed.selected_lines();
            let (s, e) = ed.lines_byte_range(l1, l2);
            let (s, e) = if e < ed.text.len() { (s, e + 1) } else if s > 0 { (s - 1, e) } else { (s, e) };
            let col = ed.line_col(ed.caret).1;
            ed.splice(s, e, "");
            let line = l1.min(ed.line_count() - 1);
            let o = ed.offset_of(line, col);
            ed.set_caret_raw(o, o);
        });
    }

    pub fn duplicate(&mut self) {
        self.want_col = None;
        self.transact(EditKind::Other, |ed| {
            if ed.has_selection() {
                let (s, e) = ed.selection();
                let t = ed.text[s..e].to_string();
                ed.splice(e, e, &t);
                ed.select_range(e, e + t.len());
            } else {
                let (l1, l2) = ed.selected_lines();
                let (s, e) = ed.lines_byte_range(l1, l2);
                let block = ed.text[s..e].to_string();
                let col = ed.caret - s;
                ed.splice(e, e, &format!("\n{block}"));
                let c = e + 1 + col;
                ed.set_caret_raw(c, c);
            }
        });
    }

    pub fn move_lines(&mut self, up: bool) {
        self.transact(EditKind::Other, |ed| {
            let (l1, l2) = ed.selected_lines();
            let total = ed.line_count();
            if up && l1 == 0 || !up && l2 + 1 >= total {
                return;
            }
            let (bs, be) = ed.lines_byte_range(l1, l2);
            let block = ed.text[bs..be].to_string();
            let (a, b) = (ed.anchor, ed.caret);
            if up {
                let ps = ed.line_start(l1 - 1);
                let prev = ed.text[ps..bs - 1].to_string();
                ed.splice(ps, be, &format!("{block}\n{prev}"));
                let d = prev.len() + 1;
                ed.caret = b - d;
                ed.anchor = a - d;
            } else {
                let ne = ed.line_end_of(be + 1);
                let next = ed.text[be + 1..ne].to_string();
                ed.splice(bs, ne, &format!("{next}\n{block}"));
                let d = next.len() + 1;
                ed.caret = b + d;
                ed.anchor = a + d;
            }
        });
    }

    pub fn join_lines(&mut self) {
        self.transact(EditKind::Other, |ed| {
            let (l1, l2) = ed.selected_lines();
            let l2 = if l2 == l1 { l1 + 1 } else { l2 };
            let mut last = None;
            for _ in l1..l2 {
                let e = ed.line_end_of(ed.line_start(l1));
                if e >= ed.text.len() {
                    break;
                }
                let mut n = e + 1;
                while ed.text[n..].starts_with([' ', '\t']) {
                    n += 1;
                }
                let ls = ed.line_start_of(e);
                let cur = &ed.text[ls..e];
                let glue = if ed.text[n..].starts_with(')') || cur.ends_with('(') || cur.is_empty() || cur.ends_with(' ') { "" } else { " " };
                ed.splice(e, n, glue);
                last = Some(e);
            }
            if let Some(e) = last {
                ed.set_caret_raw(e, e);
            }
        });
    }

    pub fn toggle_line_comment(&mut self) {
        self.transact(EditKind::Other, |ed| {
            let (l1, l2) = ed.selected_lines();
            let lines: Vec<(usize, String)> = (l1..=l2).map(|l| (ed.line_start(l), ed.line_text(l).to_string())).collect();
            let non_empty: Vec<&(usize, String)> = lines.iter().filter(|(_, t)| !t.trim().is_empty()).collect();
            if non_empty.is_empty() {
                return;
            }
            if let Some(lang) = ed.other.clone() {
                // a line comment if the language has one, else a block comment around each line (`<!-- x -->`)
                match (lang.line_comment(), lang.block_comment()) {
                    (Some(m), _) => ed.toggle_comment_with(&lines, m, ""),
                    (None, Some((a, b))) => ed.toggle_comment_with(&lines, a, b),
                    (None, None) => {}
                }
                return;
            }
            let all_commented = non_empty.iter().all(|(_, t)| t.trim_start().starts_with(';'));
            let (a0, c0) = (ed.anchor, ed.caret);
            let mut deltas: Vec<(usize, isize)> = Vec::new(); // (position, delta) for caret fixup
            if all_commented {
                for (ls, t) in lines.iter().rev() {
                    if t.trim().is_empty() {
                        continue;
                    }
                    let ind = t.len() - t.trim_start().len();
                    let rest = &t[ind..];
                    let semis = rest.bytes().take_while(|&b| b == b';').count().min(2);
                    let sp = usize::from(rest[semis..].starts_with(' '));
                    ed.splice(ls + ind, ls + ind + semis + sp, "");
                    deltas.push((ls + ind, -((semis + sp) as isize)));
                }
            } else {
                let min_indent = non_empty.iter().map(|(_, t)| t.len() - t.trim_start().len()).min().unwrap_or(0);
                for (ls, t) in lines.iter().rev() {
                    if t.trim().is_empty() {
                        continue;
                    }
                    ed.splice(ls + min_indent, ls + min_indent, ";; ");
                    deltas.push((ls + min_indent, 3));
                }
            }
            let fix = |p: usize| -> usize {
                let p = p as isize;
                let mut q = p;
                for &(pos, d) in &deltas {
                    let pos = pos as isize;
                    if d > 0 {
                        if p >= pos {
                            q += d;
                        }
                    } else if p >= pos - d {
                        q += d;
                    } else if p > pos {
                        q -= p - pos;
                    }
                }
                q.max(0) as usize
            };
            ed.anchor = fix(a0).min(ed.text.len());
            ed.caret = fix(c0).min(ed.text.len());
        });
    }

    /// Comment or uncomment `lines` (start offset, text) with the marker of a language that is not Lisp.
    fn toggle_comment_with(&mut self, lines: &[(usize, String)], marker: &str, suffix: &str) {
        let non_empty: Vec<&(usize, String)> = lines.iter().filter(|(_, t)| !t.trim().is_empty()).collect();
        let all_commented = non_empty.iter().all(|(_, t)| t.trim_start().starts_with(marker) && (suffix.is_empty() || t.trim_end().ends_with(suffix)));
        let min_indent = non_empty.iter().map(|(_, t)| t.len() - t.trim_start().len()).min().unwrap_or(0);
        let (a0, c0) = (self.anchor, self.caret);
        let mut shift: Vec<(usize, isize)> = Vec::new();
        for (ls, t) in lines.iter().rev() {
            if t.trim().is_empty() {
                continue;
            }
            if all_commented {
                let ind = t.len() - t.trim_start().len();
                if !suffix.is_empty() {
                    // the closing marker first: it is further along the line
                    let body = t.trim_end();
                    let end = ls + body.len();
                    let cut = suffix.len() + usize::from(body[..body.len() - suffix.len()].ends_with(' ') && body.len() - suffix.len() > ind + marker.len());
                    self.splice(end - cut, end, "");
                    shift.push((end - cut, -(cut as isize)));
                }
                let n = marker.len() + usize::from(t[ind + marker.len()..].starts_with(' '));
                self.splice(ls + ind, ls + ind + n, "");
                shift.push((ls + ind, -(n as isize)));
            } else {
                if !suffix.is_empty() {
                    let end = ls + t.trim_end().len();
                    let ins = format!(" {suffix}");
                    self.splice(end, end, &ins);
                    shift.push((end, ins.len() as isize));
                }
                let ins = format!("{marker} ");
                self.splice(ls + min_indent, ls + min_indent, &ins);
                shift.push((ls + min_indent, ins.len() as isize));
            }
        }
        let fix = |p: usize| -> usize {
            let mut q = p as isize;
            for &(pos, d) in &shift {
                if p >= pos {
                    q += d.max(-((p - pos) as isize));
                }
            }
            q.max(0) as usize
        };
        self.anchor = fix(a0).min(self.text.len());
        self.caret = fix(c0).min(self.text.len());
    }

    /// Tab puts a tab character in, not spaces: a makefile (its recipes must start with one) or a file whose lines are
    /// indented with tabs.
    pub fn indents_with_tabs(&self) -> bool {
        if self.other.as_ref().is_some_and(|l| l.id() == "make") {
            return true;
        }
        let (mut tabs, mut spaces) = (0usize, 0usize);
        for l in self.text.lines().take(2000) {
            match l.as_bytes().first() {
                Some(b'\t') => tabs += 1,
                Some(b' ') if l.starts_with("  ") => spaces += 1,
                _ => {}
            }
        }
        tabs > spaces
    }

    pub fn indent_lines(&mut self) {
        let tabs = self.indents_with_tabs();
        self.transact(EditKind::Other, |ed| {
            if !ed.has_selection() || ed.line_of(ed.selection().0) == ed.line_of(ed.selection().1) && !ed.selection_covers_full_line() {
                if tabs {
                    ed.replace_selection_with("\t");
                    return;
                }
                let col = ed.line_col(ed.caret).1;
                let n = INDENT_WIDTH - col % INDENT_WIDTH;
                ed.replace_selection_with(&" ".repeat(n));
                return;
            }
            let (l1, l2) = ed.selected_lines();
            let (a, c) = (ed.anchor, ed.caret);
            let pad = if tabs { "\t".to_string() } else { " ".repeat(INDENT_WIDTH) };
            let mut a2 = a;
            let mut c2 = c;
            for l in (l1..=l2).rev() {
                let ls = ed.line_start(l);
                if ed.line_text(l).trim().is_empty() {
                    continue;
                }
                ed.splice(ls, ls, &pad);
                if ls <= a {
                    a2 += pad.len();
                }
                if ls <= c {
                    c2 += pad.len();
                }
            }
            ed.anchor = a2;
            ed.caret = c2;
        });
    }

    fn selection_covers_full_line(&self) -> bool {
        let (s, e) = self.selection();
        self.line_start_of(s) == s && (self.line_end_of(e) == e || self.line_start_of(e) == e)
    }

    pub fn unindent_lines(&mut self) {
        self.transact(EditKind::Other, |ed| {
            let (l1, l2) = ed.selected_lines();
            let (a, c) = (ed.anchor, ed.caret);
            let mut a2 = a as isize;
            let mut c2 = c as isize;
            for l in (l1..=l2).rev() {
                let ls = ed.line_start(l);
                let t = ed.line_text(l);
                let n = if t.starts_with('\t') {
                    1
                } else {
                    t.bytes().take(INDENT_WIDTH).take_while(|&b| b == b' ').count()
                };
                if n == 0 {
                    continue;
                }
                ed.splice(ls, ls + n, "");
                for p in [&mut a2, &mut c2] {
                    if *p > ls as isize {
                        *p -= (n as isize).min(*p - ls as isize);
                    }
                }
            }
            ed.anchor = a2.max(0) as usize;
            ed.caret = c2.max(0) as usize;
        });
    }

    // ---------------------------------------------------------------- code commands

    /// Reindent the whole file (Reformat Code).
    pub fn reformat(&mut self) {
        if self.other.is_some() {
            return;
        }
        let new = indent::format(&self.text, self.dialect);
        self.apply_text_keeping_caret(new);
    }

    /// Reindent only the selected lines / current line (Auto-indent lines).
    pub fn auto_indent_lines(&mut self) {
        if self.other.is_some() {
            return;
        }
        let formatted = indent::format(&self.text, self.dialect);
        let (l1, l2) = self.selected_lines();
        let old_lines: Vec<&str> = self.text.split('\n').collect();
        let new_lines: Vec<&str> = formatted.split('\n').collect();
        if old_lines.len() != new_lines.len() {
            return;
        }
        let mut out = String::with_capacity(self.text.len());
        for (i, ol) in old_lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if i >= l1 && i <= l2 {
                out.push_str(new_lines[i]);
            } else {
                out.push_str(ol);
            }
        }
        self.apply_text_keeping_caret(out);
    }

    /// Splice `new` over the current text, touching only the differing middle part.
    fn splice_minimal(&mut self, new: &str) {
        let prefix = self.text.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
        let prefix = (0..=prefix).rev().find(|&i| self.text.is_char_boundary(i) && new.is_char_boundary(i)).unwrap_or(0);
        let max_suffix = self.text.len().min(new.len()) - prefix;
        let suffix = self.text.bytes().rev().zip(new.bytes().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
        let suffix = (0..=suffix).rev().find(|&i| self.text.is_char_boundary(self.text.len() - i) && new.is_char_boundary(new.len() - i)).unwrap_or(0);
        let (os, oe) = (prefix, self.text.len() - suffix);
        self.splice(os, oe, &new[prefix..new.len() - suffix]);
    }

    /// Replace the text with `new`, keeping the caret at the same line/column
    /// (adjusted for indentation changes).
    fn apply_text_keeping_caret(&mut self, new: String) {
        if new == self.text {
            return;
        }
        let new_lines: Vec<&str> = new.split('\n').collect();
        let map = |ed: &Editor, off: usize| -> (usize, usize) {
            let (line, col) = ed.line_col(off);
            let o_ind = ed.line_text(line).chars().take_while(|c| *c == ' ' || *c == '\t').count();
            let n_ind = new_lines.get(line).map_or(0, |l| l.chars().take_while(|c| *c == ' ').count());
            (line, if col <= o_ind { n_ind } else { col - o_ind + n_ind })
        };
        let (cl, cc) = map(self, self.caret);
        let (al, ac) = map(self, self.anchor);
        self.transact(EditKind::Other, |ed| {
            ed.splice_minimal(&new);
            ed.caret = ed.offset_of(cl, cc);
            ed.anchor = ed.offset_of(al, ac);
        });
    }

    pub fn structural(&mut self, op: StructuralOp) -> bool {
        let p = self.parsed();
        let caret = self.caret;
        let r: Option<Rewrite> = match op {
            StructuralOp::SlurpForward => structure::forward_slurp(&self.text, &p, caret),
            StructuralOp::BarfForward => structure::forward_barf(&self.text, &p, caret),
            StructuralOp::SlurpBackward => structure::backward_slurp(&self.text, &p, caret),
            StructuralOp::BarfBackward => structure::backward_barf(&self.text, &p, caret),
            StructuralOp::Raise => structure::raise(&self.text, &p, caret),
            StructuralOp::Splice => structure::splice(&self.text, &p, caret),
            StructuralOp::WrapRound => structure::wrap_round(&self.text, &p, caret),
            StructuralOp::KillSexp => structure::kill_sexp(&self.text, &p, caret),
        };
        let Some(r) = r else { return false };
        self.transact(EditKind::Other, |ed| {
            ed.splice_minimal(&r.text);
            ed.set_caret_raw(r.caret, r.caret);
        });
        true
    }

    pub fn sexp_move(&mut self, mv: SexpMove, extend: bool) -> bool {
        let p = self.parsed();
        let to = match mv {
            SexpMove::Forward => structure::forward_sexp(&p, self.caret),
            SexpMove::Backward => structure::backward_sexp(&p, self.caret),
            SexpMove::Up => structure::up_list(&p, self.caret),
            SexpMove::Down => structure::down_list(&p, self.caret),
        };
        match to {
            Some(o) => {
                self.move_to(o, extend, false);
                true
            }
            None => false,
        }
    }

    /// Jump to the matching bracket of the one next to the caret.
    pub fn goto_matching_bracket(&mut self, extend: bool) -> bool {
        let p = self.parsed();
        match p.matching_bracket(self.caret) {
            Some((b, m)) => {
                // opener -> just after its closer; closer -> its opener
                let target = if m > b { m + 1 } else { m };
                self.move_to(target.min(self.text.len()), extend, false);
                true
            }
            None => false,
        }
    }

    /// Source of the form that Ctrl+Enter sends to the REPL: the selection, or the top-level form at the caret.
    pub fn form_to_eval(&self) -> Option<(usize, usize)> {
        if self.has_selection() {
            return Some(self.selection());
        }
        structure::toplevel_form_at(&self.parsed(), self.caret)
    }

    /// The sexp ending at the caret (for "evaluate last expression").
    pub fn sexp_before_caret(&self) -> Option<(usize, usize)> {
        structure::sexp_before(&self.text, &self.parsed(), self.caret)
    }

    pub fn toggle_case(&mut self) {
        self.transact(EditKind::Other, |ed| {
            let (s, e) = if ed.has_selection() {
                ed.selection()
            } else {
                let (s, e) = ed.symbol_range_at(ed.caret);
                (s, e)
            };
            if s == e {
                return;
            }
            let t = ed.text[s..e].to_string();
            let up = t.to_uppercase();
            let new = if t == up { t.to_lowercase() } else { up };
            let (a, c) = (ed.anchor, ed.caret);
            ed.splice(s, e, &new);
            let _ = (a, c);
            ed.anchor = a.min(ed.text.len());
            ed.caret = c.min(ed.text.len());
        });
    }

    /// Replace a byte range (used by completion, rename, replace in file).
    pub fn replace_range(&mut self, start: usize, end: usize, with: &str) {
        self.want_col = None;
        self.transact(EditKind::Other, |ed| {
            ed.splice(start, end, with);
            let c = start + with.len();
            ed.set_caret_raw(c, c);
        });
    }

    /// Replace many non-overlapping ranges in one undo step.
    pub fn replace_ranges(&mut self, ranges: &[(usize, usize)], with: &str) {
        self.transact(EditKind::Other, |ed| {
            for &(s, e) in ranges.iter().rev() {
                ed.splice(s, e, with);
            }
            let n = ed.text.len();
            ed.caret = ed.caret.min(n);
            ed.anchor = ed.caret;
        });
    }

    /// Make edits - (start, end, new text), in order and not overlapping - in one undo step; the caret and the
    /// selection move with the text around them (a language server's rename or formatting).
    pub fn apply_edits(&mut self, edits: &[(usize, usize, String)]) {
        let map = |pos: usize| -> usize {
            let mut out = pos as isize;
            for (s, e, new) in edits {
                if *e <= pos {
                    out += new.len() as isize - (e - s) as isize;
                } else if *s < pos {
                    // inside what is replaced: as far into the new text as it was into the old
                    out += (new.len().min(pos - s)) as isize - (pos - s) as isize;
                }
            }
            out.max(0) as usize
        };
        let (caret, anchor) = (map(self.caret), map(self.anchor));
        self.want_col = None;
        self.transact(EditKind::Other, |ed| {
            for (s, e, new) in edits.iter().rev() {
                ed.splice(*s, *e, new);
            }
            let fit = |ed: &Editor, mut p: usize| {
                p = p.min(ed.text.len());
                while !ed.text.is_char_boundary(p) {
                    p -= 1;
                }
                p
            };
            let (c, a) = (fit(ed, caret), fit(ed, anchor));
            ed.set_caret_raw(c, a);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an editor from a fixture where `|` is the caret (and `[` `]` can't be used, so selections are set explicitly).
    fn ed(fixture: &str) -> Editor {
        let c = fixture.find('|').expect("fixture needs a | caret");
        let mut e = Editor::new(&fixture.replacen('|', "", 1), Dialect::CommonLisp);
        e.set_caret(c, false);
        e
    }

    fn show(e: &Editor) -> String {
        let (s, c) = (e.anchor, e.caret);
        let mut t = e.text.clone();
        if s == c {
            t.insert(c, '|');
        } else {
            let (lo, hi) = (s.min(c), s.max(c));
            t.insert(hi, '>');
            t.insert(lo, '<');
        }
        t
    }

    #[test]
    fn typing_pairs_and_overtype() {
        let mut e = ed("|");
        e.type_text("(");
        assert_eq!(show(&e), "(|)");
        e.type_text("a");
        e.type_text(")");
        assert_eq!(show(&e), "(a)|");
        e.undo();
        assert_eq!(e.text(), "", "typing with auto-pair is one undo step");
    }

    #[test]
    fn quote_pairing_and_escape() {
        let mut e = ed("|");
        e.type_text("\"");
        assert_eq!(show(&e), "\"|\"");
        e.type_text("a");
        e.type_text("\"");
        assert_eq!(show(&e), "\"a\"|", "closing quote overtypes");
        let mut e = ed("(\"a|\")");
        e.type_text("\"");
        assert_eq!(show(&e), "(\"a\"|)", "overtype at end of string");
        let mut e = ed("(\"a|b\")");
        e.type_text("\"");
        assert_eq!(show(&e), "(\"a\\\"|b\")", "escaped inside string");
    }

    #[test]
    fn no_pairing_in_comments_and_before_identifiers() {
        let mut e = ed("; foo |");
        e.type_text("(");
        assert_eq!(show(&e), "; foo (|");
        let mut e = ed("|abc");
        e.type_text("(");
        assert_eq!(show(&e), "(|abc");
        let mut e = ed("#\\|");
        e.type_text("(");
        assert_eq!(show(&e), "#\\(|");
    }

    #[test]
    fn surround_selection() {
        let mut e = ed("a b|");
        e.select_range(2, 3);
        e.type_text("(");
        assert_eq!(e.text(), "a (b)");
        assert_eq!(e.selected_text(), "b");
    }

    #[test]
    fn backspace_deletes_empty_pair() {
        let mut e = ed("(|)");
        e.backspace();
        assert_eq!(show(&e), "|");
        let mut e = ed("(a|)");
        e.backspace();
        assert_eq!(show(&e), "(|)");
        let mut e = ed("\"|\"");
        e.backspace();
        assert_eq!(show(&e), "|");
    }

    #[test]
    fn enter_indents_lisp_style() {
        let mut e = ed("(defun foo (x)|)");
        e.insert_newline();
        assert_eq!(show(&e), "(defun foo (x)\n  |)");
        let mut e = ed("(foo a|)");
        e.insert_newline();
        assert_eq!(show(&e), "(foo a\n     |)");
        let mut e = ed("  |foo");
        e.insert_newline();
        assert_eq!(show(&e), "  \n|foo");
    }

    #[test]
    fn undo_redo_and_coalescing() {
        let mut e = ed("|");
        for c in "hello".chars() {
            e.type_text(&c.to_string());
        }
        e.type_text(" ");
        for c in "world".chars() {
            e.type_text(&c.to_string());
        }
        assert_eq!(e.text(), "hello world");
        e.undo();
        assert_eq!(e.text(), "hello ", "typing coalesces up to whitespace");
        e.undo();
        assert_eq!(e.text(), "");
        assert!(!e.undo());
        e.redo();
        assert_eq!(e.text(), "hello ");
        e.redo();
        assert_eq!(e.text(), "hello world");
        e.undo();
        // new edit drops redo
        e.type_text("x");
        assert!(!e.can_redo());
    }

    #[test]
    fn modified_flag_tracks_saved_revision() {
        let mut e = ed("a|");
        assert!(!e.is_modified());
        e.type_text("b");
        assert!(e.is_modified());
        e.mark_saved();
        assert!(!e.is_modified());
        e.undo();
        assert!(e.is_modified());
    }

    #[test]
    fn vertical_movement_keeps_column() {
        let mut e = ed("abcdef\nab\nabcdef|");
        e.move_vertical(-1, false);
        assert_eq!(e.line_col(e.caret()), (1, 2));
        e.move_vertical(-1, false);
        assert_eq!(e.line_col(e.caret()), (0, 6));
        e.move_vertical(1, false);
        e.move_vertical(1, false);
        assert_eq!(e.line_col(e.caret()), (2, 6));
    }

    #[test]
    fn word_movement() {
        let mut e = ed("|foo-bar  baz");
        e.move_word_right(false);
        assert_eq!(e.caret(), 3);
        e.move_word_right(false);
        assert_eq!(e.caret(), 4);
        e.move_word_right(false);
        assert_eq!(e.caret(), 9);
        e.move_word_left(false);
        assert_eq!(e.caret(), 4);
    }

    #[test]
    fn smart_home_toggles() {
        let mut e = ed("   foo|");
        e.move_line_start(false);
        assert_eq!(e.caret(), 3);
        e.move_line_start(false);
        assert_eq!(e.caret(), 0);
        e.move_line_start(false);
        assert_eq!(e.caret(), 3);
    }

    #[test]
    fn line_operations() {
        let mut e = ed("a\nb|\nc");
        e.duplicate();
        assert_eq!(e.text(), "a\nb\nb\nc");
        assert_eq!(e.line_col(e.caret()), (2, 1));
        let mut e = ed("a\nb|\nc");
        e.delete_line();
        assert_eq!(e.text(), "a\nc");
        let mut e = ed("a\nb\nc|");
        e.delete_line();
        assert_eq!(e.text(), "a\nb");
        let mut e = ed("a\nb|\nc");
        e.move_lines(true);
        assert_eq!(e.text(), "b\na\nc");
        assert_eq!(e.line_col(e.caret()), (0, 1));
        e.move_lines(false);
        e.move_lines(false);
        assert_eq!(e.text(), "a\nc\nb");
        assert_eq!(e.line_col(e.caret()), (2, 1));
        e.move_lines(false);
        assert_eq!(e.text(), "a\nc\nb", "no-op at the end");
    }

    #[test]
    fn join_lines_works() {
        let mut e = ed("(a\n   b|)");
        e.set_caret(1, false);
        e.join_lines();
        assert_eq!(e.text(), "(a b)");
        let mut e = ed("(foo|\n  )");
        e.join_lines();
        assert_eq!(e.text(), "(foo)");
    }

    #[test]
    fn comment_toggle_roundtrip() {
        let mut e = ed("(a)\n  (b)|\n");
        e.select_range(0, 10);
        e.toggle_line_comment();
        assert_eq!(e.text(), ";; (a)\n;;   (b)\n");
        e.toggle_line_comment();
        assert_eq!(e.text(), "(a)\n  (b)\n");
        let mut e = ed("(a)|");
        e.toggle_line_comment();
        assert_eq!(e.text(), ";; (a)");
        assert_eq!(e.caret(), 6);
    }

    #[test]
    fn tab_and_shift_tab() {
        let mut e = ed("|a");
        e.indent_lines();
        assert_eq!(e.text(), "  a");
        let mut e = ed("a\nb\nc|");
        e.select_range(0, 4);
        e.indent_lines();
        assert_eq!(e.text(), "  a\n  b\nc");
        e.unindent_lines();
        assert_eq!(e.text(), "a\nb\nc");
    }

    #[test]
    fn reformat_whole_file_keeps_caret_on_token() {
        let mut e = ed("(defun f ()\n(foo|)\n)");
        e.reformat();
        assert_eq!(e.text(), "(defun f ()\n  (foo)\n  )");
        assert_eq!(show(&e), "(defun f ()\n  (foo|)\n  )");
        e.undo();
        assert_eq!(e.text(), "(defun f ()\n(foo)\n)");
    }

    #[test]
    fn auto_indent_only_selected_lines() {
        let mut e = ed("(a\nb\nc|)");
        e.set_caret(4, false);
        e.auto_indent_lines();
        assert_eq!(e.text(), "(a\n b\nc)");
    }

    #[test]
    fn structural_ops_through_editor() {
        let mut e = ed("(a |b) c");
        assert!(e.structural(StructuralOp::SlurpForward));
        assert_eq!(show(&e), "(a |b c)");
        assert!(e.structural(StructuralOp::BarfForward));
        assert_eq!(show(&e), "(a |b) c");
        e.undo();
        e.undo();
        assert_eq!(e.text(), "(a b) c");
    }

    #[test]
    fn copy_cut_paste() {
        let mut e = ed("one\ntw|o\nthree");
        assert_eq!(e.copy_text(), "two\n");
        let cut = e.cut();
        assert_eq!(cut, "two\n");
        assert_eq!(e.text(), "one\nthree");
        e.paste("x\r\ny");
        assert_eq!(e.text(), "one\nthx\nyree", "cutting a line keeps the column");
        let mut e = ed("abc|def");
        e.select_range(1, 4);
        assert_eq!(e.cut(), "bcd");
        assert_eq!(e.text(), "aef");
    }

    #[test]
    fn extend_and_shrink_selection() {
        let mut e = ed("(defun foo (x) |(+ x 1))");
        e.extend_selection();
        assert_eq!(e.selected_text(), "(+ x 1)");
        e.extend_selection();
        assert_eq!(e.selected_text(), "defun foo (x) (+ x 1)");
        e.shrink_selection();
        assert_eq!(e.selected_text(), "(+ x 1)");
        e.shrink_selection();
        assert!(!e.has_selection());
    }

    #[test]
    fn word_selection_first() {
        let mut e = ed("(hello-wor|ld 1)");
        e.extend_selection();
        assert_eq!(e.selected_text(), "hello-world");
    }

    #[test]
    fn crlf_roundtrip() {
        let e = Editor::new("a\r\nb\r\n", Dialect::CommonLisp);
        assert_eq!(e.text(), "a\nb\n");
        assert_eq!(e.line_ending, LineEnding::CrLf);
        assert_eq!(e.text_for_disk(), "a\r\nb\r\n");
    }

    #[test]
    fn line_ending_can_be_switched_and_marks_the_file_modified() {
        let mut e = Editor::new("a\r\nb\r\n", Dialect::CommonLisp);
        assert!(!e.is_modified());
        assert!(!e.set_line_ending(LineEnding::CrLf), "already CRLF: nothing to do");
        assert!(!e.is_modified());
        assert!(e.set_line_ending(LineEnding::Lf));
        assert_eq!(e.line_ending.label(), "LF");
        assert_eq!(e.text(), "a\nb\n");
        assert_eq!(e.text_for_disk(), "a\nb\n");
        assert!(e.is_modified(), "the next save has to write the new endings");
        e.mark_saved();
        assert!(e.set_line_ending(LineEnding::CrLf));
        assert_eq!(e.text_for_disk(), "a\r\nb\r\n");
        // typing and undo go on as before, the endings stay
        e.set_caret(1, false);
        e.type_text("x");
        assert_eq!(e.text_for_disk(), "ax\r\nb\r\n");
        e.undo();
        assert_eq!(e.text_for_disk(), "a\r\nb\r\n");
    }

    #[test]
    fn unicode_navigation() {
        let mut e = ed("жук|я");
        e.move_left(false);
        assert_eq!(e.caret(), "жу".len());
        e.move_right(false);
        e.move_right(false);
        assert_eq!(e.caret(), "жукя".len());
        e.backspace();
        assert_eq!(e.text(), "жук");
        assert_eq!(e.line_col(e.caret()), (0, 3));
        assert_eq!(e.offset_of(0, 2), "жу".len());
    }

    #[test]
    fn new_line_above_below() {
        let mut e = ed("(a\n  b|)");
        e.new_line_below();
        assert_eq!(e.text(), "(a\n  b)\n");
        let mut e = ed("(defun f ()\n  x|)");
        e.new_line_above();
        assert_eq!(show(&e), "(defun f ()\n  |\n  x)");
    }

    #[test]
    fn matching_bracket_and_sexp_moves() {
        let mut e = ed("(a (b)|)");
        assert!(e.goto_matching_bracket(false));
        assert_eq!(e.caret(), 3);
        let mut e = ed("|(a (b))");
        assert!(e.sexp_move(SexpMove::Forward, false));
        assert_eq!(e.caret(), 7);
        assert!(e.sexp_move(SexpMove::Backward, false));
        assert_eq!(e.caret(), 0);
    }

    #[test]
    fn form_to_eval_prefers_selection_then_toplevel() {
        let mut e = ed("(a)\n(b |c)\n");
        assert_eq!(e.form_to_eval(), Some((4, 9)));
        e.select_range(5, 6);
        assert_eq!(e.form_to_eval(), Some((5, 6)));
    }

    #[test]
    fn never_panics_on_random_edit_sequences() {
        let mut e = ed("|");
        let inputs = ["(", "\"", "a", " ", ")", "\n", "[", "'", ";", "#|", "|#", "é", "\\"];
        let mut seed = 12345u64;
        for _ in 0..3000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            match (seed >> 33) % 14 {
                0..=5 => e.type_text(inputs[(seed >> 40) as usize % inputs.len()]),
                6 => e.backspace(),
                7 => e.delete_forward(),
                8 => e.insert_newline(),
                9 => e.undo_then_redo(),
                10 => e.move_vertical(if seed & 1 == 0 { -1 } else { 1 }, seed & 2 == 0),
                11 => e.reformat(),
                12 => e.toggle_line_comment(),
                _ => {
                    e.structural(StructuralOp::SlurpForward);
                    e.extend_selection();
                }
            }
            assert!(e.text.is_char_boundary(e.caret) && e.text.is_char_boundary(e.anchor));
        }
    }

    impl Editor {
        fn undo_then_redo(&mut self) {
            self.undo();
            self.redo();
        }
    }

    /// A language of the registry that ships with the IDE.
    fn registry_lang(module: &str, id: &str) -> Arc<Language> {
        let json = std::fs::read_to_string(format!("{}/../../registry/modules/{module}/module.json", env!("CARGO_MANIFEST_DIR"))).expect("registry module");
        let m = crate::modules::Manifest::parse(&json).expect("manifest");
        Arc::new(Language::new(m.languages.into_iter().find(|l| l.id == id).expect("language")).expect("grammar"))
    }

    fn with_lang(text: &str, module: &str, id: &str) -> Editor {
        let mut e = Editor::new(text, Dialect::CommonLisp);
        e.set_language(Some(registry_lang(module, id)));
        e.set_caret(e.text().len(), false);
        e
    }

    #[test]
    fn c_files_indent_by_braces_and_comment_with_slashes() {
        let mut e = with_lang("int main(void) {", "c-cpp", "c");
        e.type_text("}");
        e.set_caret(e.text().len() - 1, false);
        e.insert_newline();
        assert_eq!(e.text(), "int main(void) {\n    \n}");
        e.type_text("x = 1; // (not lisp");
        e.insert_newline();
        assert_eq!(e.text(), "int main(void) {\n    x = 1; // (not lisp\n    \n}");
        e.type_text("}");
        assert_eq!(e.text(), "int main(void) {\n    x = 1; // (not lisp\n}\n}");
        e.undo();
        e.set_caret(20, false);
        e.toggle_line_comment();
        assert!(e.text().contains("\n    // x = 1; // (not lisp"), "{}", e.text());
        e.toggle_line_comment();
        assert!(e.text().contains("\n    x = 1; // (not lisp"), "{}", e.text());
        // the bracket inside the comment does not break matching
        let open = e.text().find('{').unwrap();
        assert_eq!(e.matching_bracket(open), Some((open, e.text().rfind('}').unwrap())));
        // Lisp reformatting leaves C alone
        let before = e.text().to_string();
        e.reformat();
        assert_eq!(e.text(), before);
    }

    #[test]
    fn makefiles_keep_the_previous_indentation() {
        let mut e = with_lang("all:\n\tmake -C x", "shell-make", "make");
        e.insert_newline();
        assert_eq!(e.text(), "all:\n\tmake -C x\n\t");
        e.set_caret(5, false);
        e.toggle_line_comment();
        assert_eq!(e.text(), "all:\n\t# make -C x\n\t");
    }

    #[test]
    fn tab_puts_a_tab_into_makefiles_and_tab_indented_files() {
        // at the start of a line, of the file, anywhere: a real tab, which recipes need
        let mut e = with_lang("", "shell-make", "make");
        e.indent_lines();
        assert_eq!(e.text(), "\t");
        let mut e = with_lang("all: main.o\ncc -o all main.o", "shell-make", "make");
        e.set_caret(12, false);
        e.indent_lines();
        assert_eq!(e.text(), "all: main.o\n\tcc -o all main.o");
        // the whole selection moves by a tab, and Shift+Tab takes it back
        e.select_range(0, e.text().len());
        e.indent_lines();
        assert_eq!(e.text(), "\tall: main.o\n\t\tcc -o all main.o");
        e.unindent_lines();
        assert_eq!(e.text(), "all: main.o\n\tcc -o all main.o");
        // Enter under a rule starts its recipe with a tab; under a variable it does not
        let mut e = with_lang("build: a.c", "shell-make", "make");
        e.insert_newline();
        assert_eq!(e.text(), "build: a.c\n\t");
        let mut e = with_lang("CFLAGS := -O2", "shell-make", "make");
        e.insert_newline();
        assert_eq!(e.text(), "CFLAGS := -O2\n");
        // a C file indented with tabs keeps to tabs, one indented with spaces to spaces
        let mut c = with_lang("int f() {\n\treturn 1;\n}\n", "c-cpp", "c");
        assert!(c.indents_with_tabs());
        c.indent_lines();
        assert!(c.text().ends_with("}\n\t"));
        assert!(!with_lang("int f() {\n    return 1;\n}\n", "c-cpp", "c").indents_with_tabs());
        assert!(is_make_rule("%.o: %.c") && is_make_rule("all:") && !is_make_rule("X = a:b") && !is_make_rule("Y ::= 1") && !is_make_rule("\tcc a:b") && !is_make_rule("# a: b"));
    }

    #[test]
    fn html_and_css_edit_with_their_own_indentation_and_comments() {
        let mut html = with_lang("<main></main>", "html", "html");
        html.set_caret(6, false);
        html.insert_newline();
        assert_eq!(html.text(), "<main>\n  \n</main>");
        html.type_text("<img src='x'>");
        html.insert_newline();
        assert_eq!(html.text(), "<main>\n  <img src='x'>\n  \n</main>");
        html.undo();
        assert_eq!(html.text(), "<main>\n  <img src='x'>\n</main>");

        let mut css = with_lang(".card {}", "css", "css");
        css.set_caret(7, false);
        css.insert_newline();
        css.type_text("background-color: red;");
        assert_eq!(css.text(), ".card {\n  background-color: red;\n}");
        let pos = css.text().find("color").unwrap() + 5;
        let (a, b) = css.identifier_range_at(pos);
        assert_eq!(&css.text()[a..b], "background-color");

        for (id, src, commented) in [("html", "<p>Hello</p>", "<!-- <p>Hello</p> -->"), ("css", "color: red;", "/* color: red; */")] {
            let mut e = with_lang(src, id, id);
            e.toggle_line_comment();
            assert_eq!(e.text(), commented);
            e.toggle_line_comment();
            assert_eq!(e.text(), src);
        }
    }

    #[test]
    fn python_indents_after_a_colon_and_comments_with_hash() {
        let mut e = with_lang("def f(x):", "python", "python");
        e.insert_newline();
        assert_eq!(e.text(), "def f(x):\n    ");
        e.type_text("return x");
        e.insert_newline();
        assert_eq!(e.text(), "def f(x):\n    return x\n", "return dedents the next line");
        e.set_caret(3, false);
        e.toggle_line_comment();
        assert!(e.text().starts_with("# def f(x):"), "{}", e.text());
    }

    #[test]
    fn markdown_lists_carry_on_and_end_on_an_empty_item() {
        let mut e = with_lang("- one", "markdown", "markdown");
        e.insert_newline();
        assert_eq!(e.text(), "- one\n- ");
        e.type_text("two");
        e.insert_newline();
        assert_eq!(e.text(), "- one\n- two\n- ");
        // Enter on the empty item ends the list: the marker goes away
        e.insert_newline();
        assert_eq!(e.text(), "- one\n- two\n\n");
        let mut n = with_lang("1. a", "markdown", "markdown");
        n.insert_newline();
        assert_eq!(n.text(), "1. a\n2. ");
        let mut q = with_lang("> quote", "markdown", "markdown");
        q.insert_newline();
        assert_eq!(q.text(), "> quote\n> ");
        // text that is not a list is just a new line, and a list marker in the middle of a line is left alone
        let mut p = with_lang("plain", "markdown", "markdown");
        p.insert_newline();
        assert_eq!(p.text(), "plain\n");
        let mut m = with_lang("- ab", "markdown", "markdown");
        m.set_caret(3, false);
        m.insert_newline();
        assert_eq!(m.text(), "- a\n- b", "splitting an item continues the list too");
    }

    #[test]
    fn languages_without_a_line_comment_wrap_lines_in_a_block_comment() {
        let mut e = with_lang("one\ntwo", "markdown", "markdown");
        e.select_all();
        e.toggle_line_comment();
        assert_eq!(e.text(), "<!-- one -->\n<!-- two -->");
        e.toggle_line_comment();
        assert_eq!(e.text(), "one\ntwo");
        // plain text has no comment: nothing happens
        let mut p = Editor::new("x", Dialect::CommonLisp);
        p.set_language(Some(Arc::new(Language::plain())));
        p.toggle_line_comment();
        assert_eq!(p.text(), "x");
    }

    #[test]
    fn highlighting_spans_are_made_once_per_revision() {
        let mut e = with_lang("int x; // c", "c-cpp", "c");
        let a = e.language_spans().unwrap();
        let b = e.language_spans().unwrap();
        assert!(Rc::ptr_eq(&a, &b), "cached");
        e.type_text("1");
        let c = e.language_spans().unwrap();
        assert!(!Rc::ptr_eq(&a, &c), "an edit makes new ones");
        assert!(Editor::new("(x)", Dialect::CommonLisp).language_spans().is_none(), "Lisp has its own");
    }

    #[test]
    fn edits_of_a_server_are_one_undo_step_and_carry_the_caret() {
        let mut e = ed("int add(int a);
int x = ad|d(1);
");
        let edits = vec![(4, 7, "sum".to_string()), (24, 27, "total".to_string())];
        e.apply_edits(&edits);
        assert_eq!(show(&e), "int sum(int a);
int x = to|tal(1);
");
        e.undo();
        assert_eq!(e.text(), "int add(int a);
int x = add(1);
");
        // reformatting before the caret moves it by what was added
        let mut e = ed("int  main(){|}");
        e.apply_edits(&[(3, 5, " ".to_string()), (11, 11, " ".to_string())]);
        assert_eq!(show(&e), "int main() {|}");
    }
}

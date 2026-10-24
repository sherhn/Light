//! Code folding of multi-line lists. A fold is identified by `(text of its first line, occurrence index)`
//! so that it survives edits elsewhere in the file.

use std::collections::{HashMap, HashSet};

use ide_core::editor::Editor;
use lisp_syntax::TokenKind;

pub type FoldKey = (String, usize);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub start_line: usize,
    pub end_line: usize,
    pub key: FoldKey,
}

/// All multi-line list regions; for every start line only the outermost region is kept.
pub fn regions(ed: &Editor) -> Vec<Region> {
    let text = ed.text();
    let starts = lisp_syntax::indent::line_starts(text);
    let line_of = |off: usize| starts.partition_point(|&s| s <= off) - 1;
    let mut by_start: HashMap<usize, usize> = HashMap::new();
    let mut add = |sl: usize, el: usize| {
        if el > sl {
            let e = by_start.entry(sl).or_insert(el);
            if el > *e {
                *e = el;
            }
        }
    };
    if let Some(lang) = ed.language() {
        // another language folds the way its definition says: braces, indentation or headings
        let spans = ed.language_spans().unwrap_or_default();
        for (a, b) in lang.fold_regions(text, &spans) {
            add(a, b);
        }
        return keyed(text, &starts, by_start);
    }
    let p = ed.parsed();
    for (i, t) in p.tokens.iter().enumerate() {
        if t.kind != TokenKind::Open {
            continue;
        }
        let Some(c) = p.partner[i] else { continue };
        add(line_of(t.start), line_of(p.tokens[c].start));
    }
    keyed(text, &starts, by_start)
}

/// Regions (start line -> end line) with keys that survive edits elsewhere in the file.
fn keyed(text: &str, starts: &[usize], by_start: HashMap<usize, usize>) -> Vec<Region> {
    let mut v: Vec<(usize, usize)> = by_start.into_iter().collect();
    v.sort_unstable();
    let mut seen: HashMap<String, usize> = HashMap::new();
    v.into_iter()
        .map(|(sl, el)| {
            let line = text[starts[sl]..].lines().next().unwrap_or("").trim().to_string();
            let n = seen.entry(line.clone()).or_insert(0);
            let key = (line, *n);
            *n += 1;
            Region { start_line: sl, end_line: el, key }
        })
        .collect()
}

/// Lines to draw, in order, with folded interiors removed.
pub fn visible_lines(total_lines: usize, regions: &[Region], folds: &HashSet<FoldKey>) -> Vec<usize> {
    if folds.is_empty() {
        return (0..total_lines).collect();
    }
    let folded: HashMap<usize, usize> = regions.iter().filter(|r| folds.contains(&r.key)).map(|r| (r.start_line, r.end_line)).collect();
    let mut out = Vec::with_capacity(total_lines);
    let mut line = 0;
    while line < total_lines {
        out.push(line);
        line = match folded.get(&line) {
            Some(&end) => end + 1,
            None => line + 1,
        };
    }
    out
}

/// Visible lines as a table. Without folds it is the identity mapping and costs nothing, even for
/// files with hundreds of thousands of lines.
pub enum Rows {
    All(usize),
    Some(Vec<usize>),
}

impl Rows {
    pub fn len(&self) -> usize {
        match self {
            Rows::All(n) => *n,
            Rows::Some(v) => v.len(),
        }
    }
    pub fn get(&self, row: usize) -> Option<usize> {
        match self {
            Rows::All(n) => (row < *n).then_some(row),
            Rows::Some(v) => v.get(row).copied(),
        }
    }
    pub fn at(&self, row: usize) -> usize {
        self.get(row).unwrap_or(0)
    }
    /// Row of `line`; a hidden line maps to the row of the fold it is hidden in.
    pub fn row_of_line(&self, line: usize) -> usize {
        match self {
            Rows::All(n) => line.min(n.saturating_sub(1)),
            Rows::Some(v) => v.partition_point(|&l| l < line).min(v.len().saturating_sub(1)),
        }
    }
}

pub fn rows(total_lines: usize, regions: &[Region], folds: &HashSet<FoldKey>) -> Rows {
    if folds.is_empty() { Rows::All(total_lines) } else { Rows::Some(visible_lines(total_lines, regions, folds)) }
}

/// The innermost region that contains `line` (start line excluded when `strict`).
pub fn region_at(regions: &[Region], line: usize) -> Option<&Region> {
    regions.iter().filter(|r| r.start_line <= line && line <= r.end_line).min_by_key(|r| r.end_line - r.start_line)
}

/// Is `line` hidden by one of the folded regions (i.e. strictly inside)?
pub fn hidden_by<'a>(regions: &'a [Region], folds: &HashSet<FoldKey>, line: usize) -> Option<&'a Region> {
    regions.iter().find(|r| folds.contains(&r.key) && r.start_line < line && line <= r.end_line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lisp_syntax::Dialect;

    fn ed(s: &str) -> Editor {
        Editor::new(s, Dialect::CommonLisp)
    }

    const SRC: &str = "(defun a ()\n  (let ((x 1))\n    x))\n(defun b () 1)\n(defun c ()\n  2)\n";

    #[test]
    fn finds_multiline_regions_outermost_per_line() {
        let r = regions(&ed(SRC));
        let spans: Vec<(usize, usize)> = r.iter().map(|x| (x.start_line, x.end_line)).collect();
        assert_eq!(spans, vec![(0, 2), (1, 2), (4, 5)]);
        assert_eq!(r[0].key, ("(defun a ()".to_string(), 0));
    }

    #[test]
    fn keys_count_duplicates() {
        let r = regions(&ed("(f\n 1)\n(f\n 2)\n"));
        assert_eq!(r[0].key, ("(f".to_string(), 0));
        assert_eq!(r[1].key, ("(f".to_string(), 1));
    }

    #[test]
    fn folding_hides_interior_lines() {
        let e = ed(SRC);
        let r = regions(&e);
        let mut folds = HashSet::new();
        assert_eq!(visible_lines(6, &r, &folds), vec![0, 1, 2, 3, 4, 5]);
        folds.insert(r[0].key.clone());
        assert_eq!(visible_lines(6, &r, &folds), vec![0, 3, 4, 5]);
        folds.insert(r[2].key.clone());
        assert_eq!(visible_lines(6, &r, &folds), vec![0, 3, 4]);
    }

    #[test]
    fn inner_fold_inside_outer_fold_is_harmless() {
        let e = ed(SRC);
        let r = regions(&e);
        let folds: HashSet<_> = [r[0].key.clone(), r[1].key.clone()].into_iter().collect();
        assert_eq!(visible_lines(6, &r, &folds), vec![0, 3, 4, 5]);
        assert!(hidden_by(&r, &folds, 2).is_some());
        assert!(hidden_by(&r, &folds, 0).is_none());
    }

    #[test]
    fn region_lookup_prefers_innermost() {
        let r = regions(&ed(SRC));
        assert_eq!(region_at(&r, 2).unwrap().start_line, 1);
        assert_eq!(region_at(&r, 0).unwrap().start_line, 0);
        assert!(region_at(&r, 3).is_none());
    }

    #[test]
    fn keys_survive_edits_elsewhere() {
        let mut e = ed(SRC);
        let key = regions(&e)[2].key.clone();
        e.set_caret(0, false);
        e.insert_text_raw(";; a new first line\n");
        let r = regions(&e);
        assert!(r.iter().any(|x| x.key == key && x.start_line == 5), "{r:?}");
    }

    #[test]
    fn unbalanced_text_is_fine() {
        assert!(regions(&ed("(defun a\n  (b\n")).is_empty());
    }
}

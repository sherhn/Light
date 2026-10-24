//! Text selection in the console panels (terminal, REPL, run output): which part of the log is selected and the
//! text it copies. Positions are a line of the log and a column counted in characters of the text as it is shown
//! (prompt and `=>` included), so what is copied is what is seen. Drawing and mouse handling are in `ui_panels`.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Where the mouse went down.
    pub anchor: Pos,
    /// Where it is now.
    pub head: Pos,
}

impl Selection {
    pub fn new(anchor: Pos, head: Pos) -> Selection {
        Selection { anchor, head }
    }

    /// (start, end) in text order, whichever way it was dragged.
    pub fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The part of `line` (`len` characters long) that is selected: `(from, to, continues)`; `continues` says the
    /// selection goes on to the next line, so the line break is selected too.
    pub fn span(&self, line: usize, len: usize) -> Option<(usize, usize, bool)> {
        let (a, b) = self.ordered();
        if self.is_empty() || line < a.line || line > b.line {
            return None;
        }
        let from = if line == a.line { a.col.min(len) } else { 0 };
        let (to, continues) = if line == b.line { (b.col.min(len), false) } else { (len, true) };
        Some((from, to, continues))
    }

    /// The selection with its ends moved inside a log of `lines` lines (`None`: nothing of it is left).
    pub fn clamped(&self, lines: usize) -> Option<Selection> {
        if lines == 0 || self.anchor.line >= lines || self.head.line >= lines { None } else { Some(*self) }
    }
}

/// Everything: from the start of the first line to the end of the last (`len(i)` characters long).
pub fn select_all(lines: usize, len: impl Fn(usize) -> usize) -> Option<Selection> {
    (lines > 0).then(|| Selection::new(Pos { line: 0, col: 0 }, Pos { line: lines - 1, col: len(lines - 1) }))
}

/// The selected text, lines joined with `\n`. `row(i)` is line `i` as it is shown.
pub fn selected_text(sel: &Selection, lines: usize, row: impl Fn(usize) -> String) -> String {
    let (a, b) = sel.ordered();
    if sel.is_empty() || lines == 0 {
        return String::new();
    }
    let mut out = String::new();
    for line in a.line..=b.line.min(lines - 1) {
        let text = row(line);
        let len = text.chars().count();
        let (from, to, _) = sel.span(line, len).unwrap_or((0, 0, false));
        if line > a.line {
            out.push('\n');
        }
        out.extend(text.chars().skip(from).take(to.saturating_sub(from)));
    }
    out
}

/// Characters that belong to a "word" for a double click: a path or a file name with its dots, slashes and colons.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || "_-./\\:~@%+=#,".contains(c)
}

/// The word at column `col` of `text` as `(from, to)` in characters; a run of blanks or one symbol if there is none.
pub fn word_range(text: &str, col: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let i = col.min(chars.len() - 1);
    let same = |c: char| -> fn(char) -> bool {
        if is_word(c) {
            is_word
        } else if c.is_whitespace() {
            char::is_whitespace
        } else {
            |_| false
        }
    };
    let belongs = same(chars[i]);
    let mut from = i;
    while from > 0 && belongs(chars[from - 1]) {
        from -= 1;
    }
    let mut to = i + 1;
    while to < chars.len() && belongs(chars[to]) {
        to += 1;
    }
    (from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    const LOG: [&str; 4] = ["$ ls", "Doxyfile  README.md", "", "sandbox-5096: 1: make: Permission denied"];

    fn row(i: usize) -> String {
        LOG[i].to_string()
    }

    #[test]
    fn order_and_emptiness() {
        let s = Selection::new(p(2, 5), p(0, 1));
        assert_eq!(s.ordered(), (p(0, 1), p(2, 5)));
        assert!(!s.is_empty());
        assert!(Selection::new(p(1, 3), p(1, 3)).is_empty());
        assert_eq!(Selection::default().span(0, 10), None);
    }

    #[test]
    fn spans_per_line() {
        let s = Selection::new(p(1, 4), p(3, 7));
        assert_eq!(s.span(0, 10), None);
        assert_eq!(s.span(1, 19), Some((4, 19, true)));
        assert_eq!(s.span(2, 0), Some((0, 0, true)));
        assert_eq!(s.span(3, 40), Some((0, 7, false)));
        assert_eq!(s.span(4, 5), None);
        // columns past the end of a short line are cut to its length
        assert_eq!(Selection::new(p(0, 1), p(0, 50)).span(0, 4), Some((1, 4, false)));
        // dragged backwards: the same
        assert_eq!(Selection::new(p(3, 7), p(1, 4)).span(1, 19), Some((4, 19, true)));
    }

    #[test]
    fn copies_what_is_shown() {
        let one = Selection::new(p(1, 0), p(1, 8));
        assert_eq!(selected_text(&one, 4, row), "Doxyfile");
        let several = Selection::new(p(0, 2), p(3, 9));
        assert_eq!(selected_text(&several, 4, row), "ls\nDoxyfile  README.md\n\nsandbox-5");
        // backwards, and past the end of the log
        assert_eq!(selected_text(&Selection::new(p(1, 8), p(1, 0)), 4, row), "Doxyfile");
        assert_eq!(selected_text(&Selection::new(p(3, 23), p(9, 0)), 4, row), "Permission denied");
        assert_eq!(selected_text(&Selection::default(), 4, row), "");
        // non-ASCII text is cut at characters, not bytes
        let ru = |_: usize| "ошибка: make не найден".to_string();
        assert_eq!(selected_text(&Selection::new(p(0, 8), p(0, 12)), 1, ru), "make");
    }

    #[test]
    fn everything_and_clamping() {
        let all = select_all(4, |i| LOG[i].chars().count()).unwrap();
        assert_eq!(all, Selection::new(p(0, 0), p(3, 40)));
        assert_eq!(selected_text(&all, 4, row), LOG.join("\n"));
        assert_eq!(select_all(0, |_| 0), None);
        assert_eq!(all.clamped(4), Some(all));
        assert_eq!(all.clamped(3), None, "the log was cleared or cut");
        assert_eq!(all.clamped(0), None);
    }

    #[test]
    fn words_for_double_click() {
        let t = "Doxyfile  /mnt/c/Users/me/os-pi: eval.c:3:12: error";
        assert_eq!(word_range(t, 3), (0, 8));
        assert_eq!(word_range(t, 8), (8, 10), "a run of blanks");
        assert_eq!(word_range(t, 14), (10, 32), "a path with its slashes");
        assert_eq!(word_range(t, 40), (33, 45), "file:line:column");
        assert_eq!(word_range(t, 999), (46, 51), "past the end: the last word");
        assert_eq!(word_range("a (b)", 2), (2, 3), "one symbol");
        assert_eq!(word_range("", 3), (0, 0));
        assert_eq!(word_range("Привет мир", 8), (7, 10));
    }
}

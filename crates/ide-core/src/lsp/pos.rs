//! Positions: the server names a place by line and character, the editor by byte offset. What a "character" is
//! depends on the encoding agreed at the start - UTF-16 code units unless the server takes UTF-8 bytes - so that `ё`
//! is one character to a UTF-16 server and two to a UTF-8 one, and an emoji two and four.

use serde::{Deserialize, Serialize};

/// How the server counts the characters of a line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    /// What the protocol assumes when nothing else was agreed.
    #[default]
    Utf16,
    Utf32,
}

impl Encoding {
    /// The encoding the server chose (`positionEncoding` of its capabilities).
    pub fn from_name(name: &str) -> Encoding {
        match name {
            "utf-8" => Encoding::Utf8,
            "utf-32" => Encoding::Utf32,
            _ => Encoding::Utf16,
        }
    }

    fn units(self, c: char) -> usize {
        match self {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf32 => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// The lines of a text, for turning offsets into positions and back. A line ends at `\n`; a `\r` before it belongs
/// to no line (the editor's own text has none, a file read from disk may).
pub struct LineMap<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineMap<'a> {
    pub fn new(text: &'a str) -> LineMap<'a> {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineMap { text, starts }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The text of line `n`, without its end.
    pub fn line(&self, n: usize) -> &'a str {
        let Some(&s) = self.starts.get(n) else { return "" };
        let e = self.starts.get(n + 1).map_or(self.text.len(), |&e| e - 1);
        self.text[s..e].strip_suffix('\r').unwrap_or(&self.text[s..e])
    }

    /// The position of byte `offset` (inside a character: of that character).
    pub fn position(&self, offset: usize, enc: Encoding) -> Position {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let start = self.starts[line];
        let before = &self.line(line)[..(offset - start).min(self.line(line).len())];
        Position { line: line as u32, character: before.chars().map(|c| enc.units(c)).sum::<usize>() as u32 }
    }

    /// The byte offset of `p`. A character past the end of its line is the end of the line, a line past the end of
    /// the text is the end of the text, and a position inside a character (half of a UTF-16 pair) is that character.
    pub fn offset(&self, p: Position, enc: Encoding) -> usize {
        let line = p.line as usize;
        let Some(&start) = self.starts.get(line) else { return self.text.len() };
        let mut left = p.character as usize;
        for (i, c) in self.line(line).char_indices() {
            let n = enc.units(c);
            if left < n {
                return start + i;
            }
            left -= n;
        }
        start + self.line(line).len()
    }

    /// The byte range of `r` (start and end in order).
    pub fn range(&self, r: Range, enc: Encoding) -> (usize, usize) {
        let (a, b) = (self.offset(r.start, enc), self.offset(r.end, enc));
        (a.min(b), a.max(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn ascii_is_the_same_in_every_encoding() {
        let m = LineMap::new("int a;\nint b;\n");
        for enc in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
            assert_eq!(m.position(11, enc), pos(1, 4));
            assert_eq!(m.offset(pos(1, 4), enc), 11);
            assert_eq!(m.position(14, enc), pos(2, 0));
        }
    }

    #[test]
    fn characters_are_counted_in_the_agreed_units() {
        // ж: 2 bytes, 1 UTF-16 unit; 😀: 4 bytes, 2 UTF-16 units
        let text = "s = \"жж😀x\";";
        let m = LineMap::new(text);
        let x = text.find('x').unwrap();
        assert_eq!(m.position(x, Encoding::Utf8), pos(0, 13));
        assert_eq!(m.position(x, Encoding::Utf16), pos(0, 9));
        assert_eq!(m.position(x, Encoding::Utf32), pos(0, 8));
        for enc in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
            assert_eq!(m.offset(m.position(x, enc), enc), x);
        }
        // half of the emoji's UTF-16 pair is the emoji
        let emoji = text.find('😀').unwrap();
        assert_eq!(m.offset(pos(0, 8), Encoding::Utf16), emoji);
    }

    #[test]
    fn positions_past_the_end_are_clamped() {
        let m = LineMap::new("ab\ncd");
        assert_eq!(m.offset(pos(0, 99), Encoding::Utf16), 2);
        assert_eq!(m.offset(pos(9, 0), Encoding::Utf16), 5);
        assert_eq!(m.position(99, Encoding::Utf16), pos(1, 2));
        assert_eq!(m.range(Range { start: pos(1, 2), end: pos(0, 1) }, Encoding::Utf8), (1, 5));
    }

    #[test]
    fn a_carriage_return_is_no_character() {
        let m = LineMap::new("ab\r\ncd");
        assert_eq!(m.line(0), "ab");
        assert_eq!(m.offset(pos(0, 5), Encoding::Utf16), 2);
        assert_eq!(m.offset(pos(1, 1), Encoding::Utf16), 5);
        assert_eq!(m.position(5, Encoding::Utf16), pos(1, 1));
    }
}

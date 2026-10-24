//! A language as it is written in a module's manifest (JSON): file names, comment markers, brackets, indentation, a
//! grammar of highlighting rules and the patterns of its outline. Nothing here is code: a language comes down to data.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// How a new line is indented.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct IndentDef {
    /// `keep` (as the line above), `braces` (one level more after `{`, `(` or `[`), `python` (after a `:` too) or
    /// `markup` (as `keep`, and Enter continues a list or a quotation), or `tags` (after opening markup tags).
    pub mode: String,
    pub size: usize,
}

impl Default for IndentDef {
    fn default() -> Self {
        IndentDef { mode: "keep".into(), size: 4 }
    }
}

/// One rule of the grammar. Exactly one of `match`, `words` or `begin`/`end` is given.
///
/// * `match`: a regular expression for one token; `class` colours all of it, or `captures` colour its groups
///   (`{"1": "keyword", "2": "function"}`) and leave the rest plain.
/// * `words`: a list of words (letters, digits, `_` and internal hyphens), whole words, coloured with `class`.
/// * `begin`/`end`: a region that may span lines (block comments, triple-quoted strings, fenced code); without an
///   `end` it runs to the end of the file.
///
/// Rules are tried at every position; the one that matches earliest wins, the first of them on a tie.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct RuleDef {
    #[serde(rename = "match")]
    pub pattern: String,
    pub words: Vec<String>,
    pub begin: String,
    pub end: String,
    pub class: String,
    pub captures: BTreeMap<String, String>,
    /// Scanning goes on right after this group instead of after the whole match (0: the whole match). A function
    /// name followed by `(` is matched as a pattern with the name in group 1 and `consume` 1, so the bracket is left to the bracket rule.
    pub consume: usize,
    pub ignore_case: bool,
    /// Optional rules applied only inside this `match` (for example, attributes and strings inside an HTML tag).
    /// The outer rule claims the whole match; inner spans are translated back to document offsets.
    pub grammar: Vec<RuleDef>,
}

/// A pattern that finds the entries of the Structure tool window (functions, classes, headings, ...). It is run over
/// the whole text with `^` and `$` meaning line starts and ends, so it can also span lines.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OutlineDef {
    pub pattern: String,
    /// `function`, `method`, `class`, `struct`, `type`, `variable`, `constant`, `macro`, `module` or `heading`.
    pub kind: String,
    /// The group holding the name.
    pub name: usize,
    /// Headings: the group that gives the level (0: none). With `level_mode` `length` it is the number of characters
    /// of the group (`##` is level 2); with `first_seen` the order in which the group's characters first appear
    /// (reStructuredText: the first kind of underline is level 1); with `setext` `=` is level 1 and anything else 2.
    pub level: usize,
    pub level_mode: String,
}

impl Default for OutlineDef {
    fn default() -> Self {
        OutlineDef { pattern: String::new(), kind: "function".into(), name: 1, level: 0, level_mode: "length".into() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct LangDef {
    pub id: String,
    pub name: String,
    /// File name extensions without the dot, any case.
    pub extensions: Vec<String>,
    /// Whole file names (`Makefile`), any case.
    pub filenames: Vec<String>,
    /// A regular expression for the first line of a file without a known name or extension (`^#!.*python`).
    pub first_line: String,
    /// What "Comment Line" puts at the start of a line (`//`, `#`, `;`); empty: none.
    pub line_comment: String,
    /// Opening and closing marker of block comments (`["/*", "*/"]`); used for "Comment Line" when there is no line
    /// comment (`["<!--", "-->"]`).
    pub block_comment: Vec<String>,
    /// Bracket pairs that match each other and are coloured by depth: `["()", "[]", "{}"]`.
    pub brackets: Vec<String>,
    /// Extra characters allowed in identifiers, e.g. `-` for CSS properties and HTML attributes.
    pub identifier_chars: String,
    /// Elements without closing tags, used by `tags` indentation and folding (case insensitive).
    pub void_tags: Vec<String>,
    pub indent: IndentDef,
    /// What can be folded: `none`, `braces`, `indent`, `headings` or `tags`.
    pub fold: String,
    /// The markup a rendered preview can show for files of this language: `markdown` or `rst` (empty: none).
    pub preview: String,
    /// The checks the editor runs on its own while you type, for problems it can find without running anything:
    /// `brackets` (a bracket that is never closed or never opened).
    pub check: Vec<String>,
    pub grammar: Vec<RuleDef>,
    pub outline: Vec<OutlineDef>,
}

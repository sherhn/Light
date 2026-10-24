//! Problems the editor finds on its own in a language's text, from what the grammar already knows: brackets that
//! do not pair up. Which checks run is up to the language (`check` in its definition).

use lisp_syntax::highlight::{Class, Span};

use crate::engine::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// An opening bracket that is never closed.
    Unclosed(char),
    /// A closing bracket that closes nothing.
    Unmatched(char),
}

impl ProblemKind {
    /// What to say about it, in English.
    pub fn message(self) -> String {
        match self {
            ProblemKind::Unclosed(c) => format!("Unclosed '{c}'"),
            ProblemKind::Unmatched(c) => format!("Unmatched '{c}'"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Byte range of the bracket.
    pub start: usize,
    pub end: usize,
    pub kind: ProblemKind,
}

impl Language {
    /// The problems the language asks to be looked for (`check`), in the order of the text. `spans` are the coloured
    /// spans of `text` (so brackets in strings and comments are not seen).
    pub fn problems(&self, text: &str, spans: &[Span]) -> Vec<Problem> {
        if !self.def.check.iter().any(|c| c == "brackets") {
            return vec![];
        }
        let pairs: Vec<(char, char)> = self
            .def
            .brackets
            .iter()
            .filter_map(|b| {
                let mut cs = b.chars();
                Some((cs.next()?, cs.next()?))
            })
            .collect();
        bracket_problems(text, spans, &pairs)
    }
}

fn bracket_problems(text: &str, spans: &[Span], pairs: &[(char, char)]) -> Vec<Problem> {
    let mut out: Vec<Problem> = vec![];
    // the open brackets, innermost last
    let mut stack: Vec<(char, usize)> = vec![];
    for s in spans.iter().filter(|s| s.class == Class::Bracket) {
        let Some(c) = text.get(s.start..).and_then(|t| t.chars().next()) else { continue };
        if pairs.iter().any(|(o, _)| *o == c) {
            stack.push((c, s.start));
        } else if let Some(&(open, _)) = pairs.iter().find(|(_, cl)| *cl == c) {
            match stack.iter().rposition(|(o, _)| *o == open) {
                Some(at) => {
                    // whatever was opened after its partner and not closed is the real problem
                    for (o, start) in stack.drain(at..).skip(1) {
                        out.push(Problem { start, end: start + o.len_utf8(), kind: ProblemKind::Unclosed(o) });
                    }
                }
                None => out.push(Problem { start: s.start, end: s.start + c.len_utf8(), kind: ProblemKind::Unmatched(c) }),
            }
        }
    }
    for (o, start) in stack {
        out.push(Problem { start, end: start + o.len_utf8(), kind: ProblemKind::Unclosed(o) });
    }
    out.sort_by_key(|p| p.start);
    out
}

#[cfg(test)]
mod tests {
    use crate::{LangDef, Language};

    fn lang(check: &[&str]) -> Language {
        Language::new(LangDef {
            id: "t".into(),
            name: "T".into(),
            brackets: vec!["()".into(), "[]".into(), "{}".into()],
            check: check.iter().map(|s| s.to_string()).collect(),
            grammar: vec![crate::RuleDef { pattern: r#""[^"\n]*""#.into(), class: "string".into(), ..Default::default() }, crate::RuleDef { pattern: "#.*$".into(), class: "comment".into(), ..Default::default() }],
            ..Default::default()
        })
        .unwrap()
    }

    fn found(l: &Language, text: &str) -> Vec<(usize, String)> {
        l.problems(text, &l.highlight(text)).into_iter().map(|p| (p.start, p.kind.message())).collect()
    }

    #[test]
    fn balanced_text_has_no_problems() {
        let l = lang(&["brackets"]);
        assert!(found(&l, "f(a[1], {b: (2)})\n").is_empty());
        // brackets in strings and comments do not count
        assert!(found(&l, "x = \"(\" # ]\n").is_empty());
    }

    #[test]
    fn unclosed_and_unmatched_brackets_are_found() {
        let l = lang(&["brackets"]);
        assert_eq!(found(&l, "f(a, [1\n"), vec![(1, "Unclosed '('".into()), (5, "Unclosed '['".into())]);
        assert_eq!(found(&l, "a)\n"), vec![(1, "Unmatched ')'".into())]);
        // a closer that skips an opener: the opener is the one that was left open
        assert_eq!(found(&l, "(a [b)\n"), vec![(3, "Unclosed '['".into())]);
        assert_eq!(found(&l, "{ ( }\n"), vec![(2, "Unclosed '('".into())]);
    }

    #[test]
    fn nothing_is_checked_unless_asked() {
        assert!(found(&lang(&[]), "f(\n").is_empty());
    }
}

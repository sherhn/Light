//! Semantic token classes used by the editor's syntax highlighting.

use crate::lexer::TokenKind;
use crate::parse::Parsed;
use crate::symbols;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Plain,
    Comment,
    String,
    Char,
    Number,
    /// `:keyword`
    Keyword,
    /// `defun`, `let`, `if`, ... (special operators / macros)
    SpecialForm,
    /// Standard library function name.
    Builtin,
    /// `nil`, `t`
    Constant,
    /// `*special*` variable
    SpecialVar,
    /// `+constant+`
    ConstName,
    /// The name being defined: `(defun NAME`
    Definition,
    /// Quote and friends.
    Prefix,
    /// A bracket. Carries rainbow depth via `Span::depth`.
    Bracket,
    Error,
    // ---- classes of the languages that modules bring (see the `light-lang` crate)
    /// A type or class name.
    Type,
    /// `#include`, `#define`, assembler directives.
    Preproc,
    /// A heading of a document.
    Heading,
    /// Strong emphasis (`**bold**`).
    Strong,
    /// Emphasis (`*italic*`).
    Emphasis,
    /// Inline code and code blocks of a document.
    Code,
    /// A link or a reference.
    Link,
    /// A quotation.
    Quote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub class: Class,
    /// Bracket nesting depth for `Class::Bracket`.
    pub depth: u32,
}

pub fn highlight(src: &str, p: &Parsed) -> Vec<Span> {
    let mut out = Vec::with_capacity(p.tokens.len());
    let mut after_definer = false;
    let mut after_open = false;
    for (i, t) in p.tokens.iter().enumerate() {
        let text = t.text(src);
        let mut depth = 0;
        let class = match t.kind {
            TokenKind::Whitespace => continue,
            TokenKind::Comment | TokenKind::BlockComment | TokenKind::UnterminatedBlockComment => Class::Comment,
            TokenKind::String | TokenKind::UnterminatedString => Class::String,
            TokenKind::Char => Class::Char,
            TokenKind::Number => Class::Number,
            TokenKind::Keyword => Class::Keyword,
            TokenKind::Prefix => Class::Prefix,
            TokenKind::Open | TokenKind::Close => {
                depth = p.depth[i];
                if p.partner[i].is_none() { Class::Error } else { Class::Bracket }
            }
            TokenKind::Symbol => {
                if after_definer {
                    Class::Definition
                } else if after_open && symbols::is_keyword(text) {
                    Class::SpecialForm
                } else if symbols::is_constant(text) {
                    Class::Constant
                } else if symbols::is_special_variable(text) {
                    Class::SpecialVar
                } else if symbols::is_constant_name(text) {
                    Class::ConstName
                } else if after_open && symbols::is_builtin_function(text) {
                    Class::Builtin
                } else if symbols::is_keyword(text) && !text.contains('/') && text.starts_with("def") {
                    Class::SpecialForm
                } else {
                    Class::Plain
                }
            }
        };
        let was_open = after_open;
        let was_definer = after_definer;
        after_definer = false;
        after_open = t.kind == TokenKind::Open;
        if t.kind == TokenKind::Symbol && was_open && !was_definer {
            let low = symbols::strip_package(text).to_ascii_lowercase();
            if low.starts_with("def") && symbols::is_keyword(&low) || matches!(low.as_str(), "define" | "defn" | "ns") {
                after_definer = true;
            }
        }
        if t.kind == TokenKind::Prefix && (was_definer) {
            after_definer = true;
        }
        out.push(Span { start: t.start, end: t.end, class, depth });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::Dialect;
    use crate::parse::parse;

    fn classes(src: &str) -> Vec<(&str, Class)> {
        let p = parse(src, Dialect::CommonLisp);
        highlight(src, &p).into_iter().map(|s| (&src[s.start..s.end], s.class)).collect()
    }

    #[test]
    fn defun_name_is_a_definition() {
        let c = classes("(defun foo (x) (car x))");
        assert!(c.contains(&("defun", Class::SpecialForm)));
        assert!(c.contains(&("foo", Class::Definition)));
        assert!(c.contains(&("car", Class::Builtin)));
        assert!(c.contains(&("x", Class::Plain)));
    }

    #[test]
    fn variables_constants_keywords() {
        let c = classes("(setf *x* nil +y+ :kw 1 \"s\" ; c\n)");
        assert!(c.contains(&("*x*", Class::SpecialVar)));
        assert!(c.contains(&("nil", Class::Constant)));
        assert!(c.contains(&("+y+", Class::ConstName)));
        assert!(c.contains(&(":kw", Class::Keyword)));
        assert!(c.contains(&("1", Class::Number)));
        assert!(c.contains(&("\"s\"", Class::String)));
        assert!(c.contains(&("; c", Class::Comment)));
    }

    #[test]
    fn unmatched_brackets_are_errors_and_depth_is_reported() {
        let src = "((a)))";
        let p = parse(src, Dialect::CommonLisp);
        let h = highlight(src, &p);
        let brackets: Vec<_> = h.iter().filter(|s| matches!(s.class, Class::Bracket | Class::Error)).collect();
        assert_eq!(brackets.len(), 5);
        assert_eq!(brackets[0].depth, 0);
        assert_eq!(brackets[1].depth, 1);
        assert_eq!(brackets[4].class, Class::Error);
    }

    #[test]
    fn variable_named_like_function_is_plain_when_not_in_call_position() {
        let c = classes("(foo list)");
        assert!(c.contains(&("list", Class::Plain)));
    }
}

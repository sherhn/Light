//! Finding where a name is defined by reading the text, whatever the language: the last resort of Go to Declaration
//! when the outline of a language does not know the definition (a C function whose name starts its line, a typedef'd
//! struct, a module-level variable, a language without an outline). It looks for the shapes definitions have in most
//! languages - a defining word before the name (`def`, `class`, `fn`, `struct`, `#define`...), a function header, a
//! label, an assignment at the start of a line - and leaves the uses alone.

use regex::{Regex, RegexBuilder};

/// Where a definition of the name was found: 0-based line and byte offset of the name in the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefLine {
    pub line: usize,
    pub offset: usize,
    /// `function`, `class`, `variable`... as far as the shape tells.
    pub kind: &'static str,
}

/// The patterns that find definitions of one name.
pub struct DefFinder {
    rules: Vec<(Regex, &'static str)>,
}

/// Words that introduce a definition of the name that follows them, and what they define.
const KEYWORDS: [(&str, &str); 6] = [
    (r"def|fn|fun|func|function|sub|proc|procedure|method", "function"),
    (r"class|interface|trait|protocol|object|record", "class"),
    (r"struct|union|enum", "struct"),
    (r"type|typedef|typealias|using", "type"),
    (r"module|namespace|package|mod|impl", "module"),
    (r"let|var|val|const|static|global|local|my|our|export", "variable"),
];

impl DefFinder {
    /// `None` when `name` is not something a definition can have (empty, or with spaces).
    pub fn new(name: &str) -> Option<DefFinder> {
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return None;
        }
        let n = format!("(?P<n>{})", regex::escape(name));
        // the name stands alone: no letter, digit, `_` (or `-` of a Lisp name) is glued to it
        let end = r"(?:$|[^\w-])";
        let mut rules: Vec<(String, &'static str)> = Vec::new();
        for (words, kind) in KEYWORDS {
            rules.push((format!(r"(?:^|[^\w.$-])(?:{words})(?:[ \t]+(?:mut|async|static|class|struct|enum|const|inline|unsafe|extern|pub|export|default)\b)*[ \t]+[\*&]?{n}{end}"), kind));
        }
        rules.extend([
            // a JavaScript-like `name: function`, `name = (...) =>`, before the assignments below take it for a variable
            (format!(r"(?:^|[^\w.$])\b{n}[ \t]*[:=][ \t]*(?:async[ \t]+)?(?:function\b|\([^)\n]*\)[ \t]*=>)"), "function"),
            // C and its kin: `int *add(`, `static void Foo::bar(`, a prototype `int f(void);`
            (format!(r"^(?P<pre>[A-Za-z_][\w \t\*&:<>,~]*?)[ \t\*&:]{n}[ \t]*\("), "function"),
            // a name that starts its line, its body after the parameters (GNU style: the type is on the line above)
            (format!(r"^{n}[ \t]*\([^;{{}}]*\)\s*\{{"), "function"),
            (format!(r"^[ \t]*#[ \t]*define[ \t]+{n}{end}"), "macro"),
            // `} Name;` closing a typedef'd struct or enum
            (format!(r"^\}}[ \t]*\*?{n}[ \t]*[;,]"), "struct"),
            // `NAME = ...` / `NAME: int = ...` / `NAME := ...` at the start of a line (Python, make, shell, config)
            (format!(r"^{n}[ \t]*(?::[^=\n]*)?(?::|\+|\?)?=(?:[^=]|$)"), "variable"),
            // a label of assembly or a target of make
            (format!(r"^[ \t]*{n}:(?:[^=:]|$)"), "function"),
            // Lisp: `(defun name`, `(defclass name`, `(define (name`
            (format!(r"\(def[\w-]*[ \t]+\(?{n}{end}"), "function"),
        ]);
        let rules = rules.into_iter().filter_map(|(p, k)| RegexBuilder::new(&p).multi_line(true).build().ok().map(|r| (r, k))).collect();
        Some(DefFinder { rules })
    }

    /// The definitions of the name in `text`. `dead` says whether a byte offset is in a comment or a string.
    pub fn find(&self, text: &str, dead: impl Fn(usize) -> bool) -> Vec<DefLine> {
        // what comes before a call, not before the name of a function being defined: `return f(x)`, `new Foo()`
        const NOT_TYPES: [&str; 22] = ["return", "await", "new", "throw", "delete", "yield", "else", "case", "do", "goto", "print", "echo", "assert", "raise", "not", "and", "or", "in", "is", "if", "while", "call"];
        let mut out: Vec<DefLine> = Vec::new();
        for (re, kind) in &self.rules {
            for c in re.captures_iter(text) {
                let Some(m) = c.name("n") else { continue };
                if dead(m.start()) || out.iter().any(|d| d.offset == m.start()) {
                    continue;
                }
                if c.name("pre").is_some_and(|p| p.as_str().split(|ch: char| !(ch.is_alphanumeric() || ch == '_')).any(|w| NOT_TYPES.contains(&w))) {
                    continue;
                }
                let line = text[..m.start()].matches('\n').count();
                out.push(DefLine { line, offset: m.start(), kind });
            }
        }
        out.sort_by_key(|d| d.offset);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(name: &str, text: &str) -> Vec<(usize, &'static str)> {
        DefFinder::new(name).unwrap().find(text, |_| false).into_iter().map(|d| (d.line, d.kind)).collect()
    }

    #[test]
    fn keywords_introduce_definitions() {
        assert_eq!(lines("greet", "import x\n\ndef greet(name):\n    return greet(name)\n"), [(2, "function")]);
        assert_eq!(lines("Shape", "class Shape(Base):\n    pass\nShape()\n"), [(0, "class")]);
        assert_eq!(lines("run", "pub async fn run() {}\nfn main() { run(); }\n"), [(0, "function")]);
        assert_eq!(lines("Point", "pub struct Point { x: i32 }\nlet p = Point { x: 1 };\n"), [(0, "struct")]);
        assert_eq!(lines("LIMIT", "const LIMIT = 10;\nif (x > LIMIT) {}\n"), [(0, "variable")]);
        assert_eq!(lines("Config", "interface Config {}\ntype Config2 = {}\n"), [(0, "class")]);
    }

    #[test]
    fn c_functions_macros_and_typedefs() {
        let c = "#define MAX(a, b) ((a) > (b))\n\ntypedef struct {\n    int x;\n} Vec2;\n\nstatic int\nadd(int a, int b)\n{\n    return MAX(a, b);\n}\n\nint *make(void) {\n    add(1, 2);\n    Vec2 v;\n}\n";
        assert_eq!(lines("MAX", c), [(0, "macro")]);
        assert_eq!(lines("Vec2", c), [(4, "struct")]);
        assert_eq!(lines("add", c), [(7, "function")], "a name that starts its line, not the call");
        assert_eq!(lines("make", c), [(12, "function")]);
        // a prototype in a header is where the function is declared
        assert_eq!(lines("open_file", "int open_file(const char *name);\n"), [(0, "function")]);
        assert_eq!(lines("Bar::draw", "void Bar::draw() {}"), [(0, "function")]);
    }

    #[test]
    fn labels_assignments_lisp_and_javascript() {
        assert_eq!(lines("start", "section .text\nstart:\n    call start\n"), [(1, "function")]);
        assert_eq!(lines("CFLAGS", "CFLAGS := -O2\nall:\n\tcc $(CFLAGS)\n"), [(0, "variable")]);
        assert_eq!(lines("TIMEOUT", "TIMEOUT: int = 5\nif TIMEOUT == 5: pass\n"), [(0, "variable")]);
        assert_eq!(lines("make-point", "(defun make-point (x y)\n  (list x y))\n(make-point 1 2)\n"), [(0, "function")]);
        assert_eq!(lines("square", "(define (square x) (* x x))\n"), [(0, "function")]);
        assert_eq!(lines("handler", "const obj = {\n  handler: function (e) {},\n};\nconst cb = (x) => handler(x);\n"), [(1, "function")]);
        assert_eq!(lines("onClick", "onClick = (e) => {}\n"), [(0, "function")]);
    }

    #[test]
    fn uses_and_longer_names_are_not_definitions() {
        assert!(lines("greet", "greet_all()\nx = greeting\nprint(greet)\n").is_empty());
        assert!(lines("add", "    add(1, 2);\nreturn add(x);\n").is_empty());
        assert!(lines("x", "if x == 1:\n    pass\n").is_empty(), "a comparison is not an assignment");
        let f = DefFinder::new("f").unwrap();
        let text = "# def f(): not this\ndef f(): pass\n";
        let comment_end = text.find('\n').unwrap();
        assert_eq!(f.find(text, |at| at < comment_end).len(), 1, "the one in the comment is left out");
        assert!(DefFinder::new("").is_none() && DefFinder::new("a b").is_none());
    }
}

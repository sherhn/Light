use std::path::Path;

use lisp_syntax::highlight::Class;

use crate::*;

const PY: &str = r##"{
  "id": "python", "name": "Python", "extensions": ["py"], "first_line": "^#!.*python",
  "line_comment": "#", "brackets": ["()", "[]", "{}"], "indent": {"mode": "python", "size": 4}, "fold": "indent",
  "grammar": [
    {"begin": "\"\"\"", "end": "\"\"\"", "class": "string"},
    {"match": "#.*$", "class": "comment"},
    {"match": "\"(?:[^\"\\\\\\n]|\\\\.)*\"?", "class": "string"},
    {"match": "\\b(def|class)\\s+(\\w+)", "captures": {"1": "keyword", "2": "function"}},
    {"words": ["if", "else", "return", "pass", "import", "for", "in"], "class": "keyword"},
    {"match": "\\b\\d+(?:\\.\\d+)?\\b", "class": "number"}
  ],
  "outline": [
    {"pattern": "^\\s*def\\s+(\\w+)", "kind": "function"},
    {"pattern": "^class\\s+(\\w+)", "kind": "class"}
  ]
}"##;

const C: &str = r##"{
  "id": "c", "name": "C", "extensions": ["c", "h"], "line_comment": "//", "block_comment": ["/*", "*/"],
  "brackets": ["()", "[]", "{}"], "indent": {"mode": "braces", "size": 4}, "fold": "braces",
  "grammar": [
    {"begin": "/\\*", "end": "\\*/", "class": "comment"},
    {"match": "//.*$", "class": "comment"},
    {"match": "\"(?:[^\"\\\\\\n]|\\\\.)*\"?", "class": "string"},
    {"match": "^\\s*#\\s*\\w+", "class": "preproc"},
    {"words": ["int", "char", "void"], "class": "type"},
    {"words": ["if", "else", "while", "return"], "class": "keyword"}
  ]
}"##;

const MD: &str = r##"{
  "id": "markdown", "name": "Markdown", "extensions": ["md"], "block_comment": ["<!--", "-->"],
  "indent": {"mode": "markup"}, "fold": "headings",
  "grammar": [
    {"begin": "^```", "end": "^```", "class": "code"},
    {"match": "^#{1,6}\\s.*$", "class": "heading"},
    {"match": "`[^`\\n]+`", "class": "code"},
    {"match": "\\*\\*[^*\\n]+\\*\\*", "class": "strong"}
  ],
  "outline": [{"pattern": "^(#{1,6})\\s+(.+?)\\s*$", "kind": "heading", "name": 2, "level": 1}]
}"##;

const RST: &str = r##"{
  "id": "rst", "name": "reStructuredText", "extensions": ["rst"], "indent": {"mode": "markup"}, "fold": "headings",
  "outline": [{"pattern": "^([^\\s.][^\\n]*)\\n([=\\-~^\"#*+]{3,})[ \\t]*$", "kind": "heading", "name": 1, "level": 2, "level_mode": "first_seen"}]
}"##;

fn lang(json: &str) -> Language {
    Language::new(serde_json::from_str(json).expect("json")).expect("grammar")
}

/// (class, text) of every span.
fn spans(l: &Language, text: &str) -> Vec<(Class, String)> {
    l.highlight(text).iter().map(|s| (s.class, text[s.start..s.end].to_string())).collect()
}

#[test]
fn python_tokens() {
    let py = lang(PY);
    let src = "def f(x):  # note\n    return x + 3.5 if x else \"a\\\"b\"\n";
    let got = spans(&py, src);
    let has = |c: Class, t: &str| got.iter().any(|(k, s)| *k == c && s == t);
    assert!(has(Class::SpecialForm, "def") && has(Class::Definition, "f"), "{got:?}");
    assert!(has(Class::Comment, "# note"));
    assert!(has(Class::SpecialForm, "return") && has(Class::SpecialForm, "if") && has(Class::SpecialForm, "else"));
    assert!(has(Class::Number, "3.5"));
    assert!(has(Class::String, "\"a\\\"b\""), "an escaped quote does not end the string: {got:?}");
    // sorted, not overlapping
    let sp = py.highlight(src);
    assert!(sp.windows(2).all(|w| w[0].end <= w[1].start));
}

#[test]
fn rules_are_tried_by_position_then_order() {
    let py = lang(PY);
    // a `#` inside a string is not a comment, a keyword inside a comment is not a keyword
    let got = spans(&py, "x = \"a # b\"  # if else\n");
    assert!(got.contains(&(Class::String, "\"a # b\"".to_string())), "{got:?}");
    assert!(got.contains(&(Class::Comment, "# if else".to_string())));
    assert!(!got.iter().any(|(k, _)| *k == Class::SpecialForm));
    // identifiers that contain a keyword are not keywords
    assert!(spans(&py, "ifx = pass_1\n").is_empty() || spans(&py, "ifx = pass_1\n").iter().all(|(k, _)| *k == Class::Bracket));
}

#[test]
fn scoped_grammars_keep_offsets_bracket_depth_and_vocabulary() {
    let l = lang(r#"{"id":"scoped","brackets":["{}","()"],"identifier_chars":"-",
        "grammar":[{"match":"value:(?:\"[^\"]*\"|[^;}\"])+","class":"plain","grammar":[
            {"match":"\"[^\"]*\"","class":"string"},
            {"words":["background-color"],"class":"argument"}
        ]}]}"#);
    let src = "{ value:(background-color \"}\"); background-color }";
    let sp = l.highlight(src);
    assert_eq!(sp.iter().filter(|s| s.class == Class::Keyword).count(), 1, "outside the scope is plain");
    assert_eq!(sp.iter().filter(|s| s.class == Class::Bracket).map(|s| s.depth).collect::<Vec<_>>(), [0, 1, 1, 0]);
    assert_eq!(l.matching_bracket(src, &sp, 0), Some((0, src.len() - 1)));
    assert_eq!(l.words(), &[("background-color".to_string(), Class::Keyword)]);
    assert!(l.is_identifier_char('-') && !Language::plain().is_identifier_char('-'));
    for s in &sp {
        assert!(src.is_char_boundary(s.start) && src.is_char_boundary(s.end));
    }
    assert!(sp.windows(2).all(|w| w[0].end <= w[1].start));
}

#[test]
fn scoped_grammar_errors_are_reported_at_the_outer_rule() {
    for json in [
        r#"{"grammar":[{"begin":"<","end":">","class":"plain","grammar":[{"words":["x"],"class":"keyword"}]}]}"#,
        r#"{"grammar":[{"match":"x","grammar":[{"match":"(","class":"string"}]}]}"#,
        r#"{"grammar":[{"match":"x","consume":1,"grammar":[{"words":["x"],"class":"keyword"}]}]}"#,
    ] {
        let err = Language::new(serde_json::from_str(json).unwrap()).err().expect("invalid grammar");
        assert!(err.contains("rule 1"), "{err}");
    }
}

#[test]
fn regions_span_lines_and_may_be_unterminated() {
    let py = lang(PY);
    let src = "a = \"\"\"one\ntwo\n\"\"\"\nb = 1\n";
    let got = spans(&py, src);
    assert!(got.contains(&(Class::String, "\"\"\"one\ntwo\n\"\"\"".to_string())), "{got:?}");
    assert!(got.contains(&(Class::Number, "1".to_string())));
    // never closed: to the end of the file, and the highlighter still ends
    let got = spans(&py, "x = \"\"\"never\nclosed");
    assert_eq!(got.last().unwrap().1, "\"\"\"never\nclosed");
    let c = lang(C);
    let got = spans(&c, "int /* open\nstill */ x;");
    assert!(got.contains(&(Class::Comment, "/* open\nstill */".to_string())), "{got:?}");
}

#[test]
fn brackets_have_depth_and_skip_strings() {
    let c = lang(C);
    let src = "f(a[1], \"(\") { }";
    let sp = c.highlight(src);
    let depths: Vec<(char, u32)> = sp.iter().filter(|s| s.class == Class::Bracket).map(|s| (src[s.start..].chars().next().unwrap(), s.depth)).collect();
    assert_eq!(depths, [('(', 0), ('[', 1), (']', 1), (')', 0), ('{', 0), ('}', 0)]);
    // the paren inside the string is not a bracket: `(` at 1 pairs with `)` at 12
    let open = src.find('(').unwrap();
    let close = src.find(')').unwrap();
    assert_eq!(c.matching_bracket(src, &sp, open), Some((open, close)));
    assert_eq!(c.matching_bracket(src, &sp, close + 1), Some((open, close)), "just after a bracket counts");
    assert_eq!(c.matching_bracket(src, &sp, 7), None, "not next to a bracket");
}

#[test]
fn preprocessor_and_types() {
    let c = lang(C);
    let got = spans(&c, "#include <stdio.h>\nint main(void) { return 0; }\n");
    assert!(got.contains(&(Class::Preproc, "#include".to_string())), "{got:?}");
    assert!(got.contains(&(Class::Type, "int".to_string())) && got.contains(&(Class::Type, "void".to_string())));
    assert!(got.contains(&(Class::SpecialForm, "return".to_string())));
}

#[test]
fn outline_of_code_and_headings() {
    let py = lang(PY);
    let src = "class A:\n    def m(self):\n        pass\n\ndef f():\n    pass\n";
    let o = py.outline(src);
    let names: Vec<(&str, usize)> = o.iter().map(|d| (d.name.as_str(), d.line)).collect();
    assert_eq!(names, [("A", 0), ("m", 1), ("f", 4)]);
    assert_eq!(&src[o[1].name_start..o[1].name_end], "m");

    let md = lang(MD);
    let doc = "# Title\n\ntext\n\n## Part\n\n### Deep\n\n## Next\n";
    let o = md.outline(doc);
    assert_eq!(o.iter().map(|d| (d.name.as_str(), d.operator.as_str())).collect::<Vec<_>>(), [("Title", "h1"), ("Part", "h2"), ("Deep", "h3"), ("Next", "h2")]);

    // reStructuredText: the underline says it, and the first kind of underline is the top level
    let rst = lang(RST);
    let doc = "Title\n=====\n\nSection\n-------\n\nSub\n~~~\n\nOther\n=====\n";
    let o = rst.outline(doc);
    assert_eq!(o.iter().map(|d| (d.name.as_str(), d.operator.as_str())).collect::<Vec<_>>(), [("Title", "h1"), ("Section", "h2"), ("Sub", "h3"), ("Other", "h1")]);
}

#[test]
fn folding_by_braces_indent_and_headings() {
    let c = lang(C);
    let src = "int main() {\n  if (x) {\n    y;\n  }\n}\n";
    let sp = c.highlight(src);
    assert_eq!(c.fold_regions(src, &sp), [(0, 4), (1, 3)]);

    let py = lang(PY);
    let src = "class A:\n    def m(self):\n        a = 1\n        b = 2\n\n    def n(self):\n        pass\nx = 1\n";
    assert_eq!(py.fold_regions(src, &[]), [(0, 6), (1, 3), (5, 6)]);

    let md = lang(MD);
    let doc = "# A\ntext\n\n## B\nmore\n\n# C\nlast\n";
    assert_eq!(md.fold_regions(doc, &[]), [(0, 4), (3, 4), (6, 7)]);
}

#[test]
fn indentation_of_new_lines() {
    let c = lang(C);
    assert_eq!(c.indent_for("int main() {", ""), "    ");
    assert_eq!(c.indent_for("    x = 1;", ""), "    ");
    assert_eq!(c.indent_for("    if (a) { // open", ""), "        ", "a comment after the brace does not matter");
    assert_eq!(c.indent_for("    foo();", "}"), "", "a closing brace typed first goes back a level");
    let py = lang(PY);
    assert_eq!(py.indent_for("def f():", ""), "    ");
    assert_eq!(py.indent_for("    return x", ""), "");
    assert_eq!(py.indent_for("    x = [", ""), "        ");
    assert_eq!(py.indent_for("\tif x:", ""), "\t\t", "tabs stay tabs");
    let md = lang(MD);
    assert_eq!(md.indent_for("  text", ""), "  ");
}

#[test]
fn lists_and_quotations_continue() {
    let md = lang(MD);
    let cont = |l: &str| md.continue_line(l);
    assert_eq!(cont("- item"), Some(Continuation { insert: "- ".into(), end_list: false }));
    assert_eq!(cont("  * nested"), Some(Continuation { insert: "  * ".into(), end_list: false }));
    assert_eq!(cont("9. nine"), Some(Continuation { insert: "10. ".into(), end_list: false }));
    assert_eq!(cont("3) three"), Some(Continuation { insert: "4) ".into(), end_list: false }));
    assert_eq!(cont("- [x] done"), Some(Continuation { insert: "- [ ] ".into(), end_list: false }));
    assert_eq!(cont("> quoted"), Some(Continuation { insert: "> ".into(), end_list: false }));
    assert_eq!(cont("- "), Some(Continuation { insert: String::new(), end_list: true }), "an empty item ends the list");
    assert_eq!(cont("> "), Some(Continuation { insert: String::new(), end_list: true }));
    assert_eq!(cont("plain text"), None);
    assert_eq!(lang(PY).continue_line("- item"), None, "only documents continue lists");
}

#[test]
fn comment_markers() {
    assert_eq!(lang(PY).line_comment(), Some("#"));
    assert_eq!(lang(C).block_comment(), Some(("/*", "*/")));
    let md = lang(MD);
    assert_eq!((md.line_comment(), md.block_comment()), (None, Some(("<!--", "-->"))));
    assert!(Language::plain().is_plain() && Language::plain().highlight("anything (1)").is_empty());
}

#[test]
fn files_find_their_language() {
    let mut set = LanguageSet::new();
    for j in [PY, C, MD] {
        set.add(serde_json::from_str(j).unwrap()).unwrap();
    }
    assert_eq!(set.for_path(Path::new("src/main.PY"), None).id(), "python");
    assert_eq!(set.for_path(Path::new("a/b.h"), None).id(), "c");
    assert_eq!(set.for_path(Path::new("README.md"), None).id(), "markdown");
    assert_eq!(set.for_path(Path::new("notes.xyz"), None).id(), "plain");
    assert_eq!(set.for_path(Path::new("script"), Some("#!/usr/bin/env python3")).id(), "python", "a shebang decides when the name does not");
    assert!(set.find(Path::new("notes.xyz"), None).is_none());
    // the same id replaces, remove forgets
    set.add(serde_json::from_str(PY).unwrap()).unwrap();
    assert_eq!(set.all().len(), 3);
    set.remove("python");
    assert_eq!(set.for_path(Path::new("a.py"), None).id(), "plain");
}

#[test]
fn bad_grammars_are_refused_with_a_reason() {
    let err = |json: &str| Language::new(serde_json::from_str(json).unwrap()).err().expect("must fail");
    assert!(err(r#"{"id":"x","grammar":[{"match":"(","class":"comment"}]}"#).contains("rule 1"));
    assert!(err(r#"{"id":"x","grammar":[{"match":"a*","class":"comment"}]}"#).contains("empty string"));
    assert!(err(r#"{"id":"x","grammar":[{"match":"a","class":"sparkles"}]}"#).contains("unknown class"));
    assert!(err(r#"{"id":"x","grammar":[{"words":["a.b"],"class":"keyword"}]}"#).contains("letters"));
    assert!(err(r#"{"id":"x","grammar":[{"match":"a(b)","captures":{"2":"keyword"}}]}"#).contains("no group 2"));
    assert!(err(r#"{"id":"x","grammar":[{"class":"keyword"}]}"#).contains("none of"));
    assert!(err(r#"{"id":"x","brackets":["("]}"#).contains("two characters"));
    assert!(err(r#"{"id":"x","indent":{"mode":"zigzag"}}"#).contains("indent mode"));
    assert!(err(r#"{"id":"x","outline":[{"pattern":"a(b)","kind":"function","name":2}]}"#).contains("no such group"));
}

#[test]
fn large_text_highlights_quickly() {
    let c = lang(C);
    let line = "if (value > 10) { return count + \"text\"; } /* note */\n";
    let text = line.repeat(20_000);
    let t = std::time::Instant::now();
    let n = c.highlight(&text).len();
    assert!(n > 100_000);
    assert!(t.elapsed().as_secs_f32() < 5.0, "{:?} for {} bytes", t.elapsed(), text.len());
}

//! The registry that ships with the IDE (`registry/`): every module is valid, the index is current, and each language
//! highlights real code the way a person would expect.

use std::path::PathBuf;

use light_lang::{Language, LanguageSet};
use ide_core::modules::{Index, Manifest, Source};
use ide_core::modules::sha256::sha256_hex;
use lisp_syntax::highlight::Class;

fn registry_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../registry")
}

fn manifests() -> Vec<Manifest> {
    let index = Source::Dir(registry_dir()).fetch_index().expect("registry/index.json");
    index.modules.iter().map(|e| Manifest::parse(&String::from_utf8(Source::Dir(registry_dir()).fetch(&format!("{}/module.json", e.path)).unwrap()).unwrap()).unwrap_or_else(|er| panic!("{}: {er}", e.id))).collect()
}

fn language(id: &str) -> Language {
    let def = manifests().into_iter().flat_map(|m| m.languages).find(|l| l.id == id).unwrap_or_else(|| panic!("no language {id}"));
    Language::new(def).unwrap()
}

#[test]
fn javascript_handles_modern_syntax_outline_and_node_commands() {
    let source = "// note\nexport async function greet(name) {\n  const re = /hello+/gi;\n  return `Hello ${name}`;\n}\nexport class Client {}\nconst double = (x) => x * 2;\n";
    let spans = hl("javascript", source);
    assert!(has(&spans, Class::Comment, "// note"));
    assert!(has(&spans, Class::String, "`Hello ${name}`"), "{spans:?}");
    assert!(has(&spans, Class::String, "/hello+/gi"), "{spans:?}");
    let language = language("javascript");
    assert_eq!(language.outline(source).iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["greet", "Client", "double"]);
    assert_eq!(language.indent_for("function main() {", ""), "  ");
    assert!(!language.fold_regions(source, &language.highlight(source)).is_empty());
    assert!(language.words().iter().any(|(w, _)| w == "Promise"));
    let module = manifests().into_iter().find(|m| m.id == "javascript").unwrap();
    assert!(module.run.iter().any(|r| r.command.contains("node --watch")));
    assert!(module.run.iter().any(|r| r.command.contains("npm --prefix") && r.command.ends_with("run build")));
    let check = module.check.first().unwrap();
    let problems = ide_core::modules::check::problems(&ide_core::modules::check::compile(&check.pattern).unwrap(), "/project/bad.js:3\nconst = ;\n      ^\n\nSyntaxError: Unexpected token '='\n", 10);
    assert_eq!(problems[0].line, 3);
    assert!(problems[0].message.contains("Unexpected token"));
}

#[test]
fn docker_highlights_stages_and_compose_services_and_exposes_lifecycle_actions() {
    let text = "FROM node:22 AS builder\nWORKDIR /app\nCOPY --from=builder /app/dist /app\nENV PORT=${PORT}\n";
    let spans = hl("dockerfile", text);
    assert!(has(&spans, Class::SpecialForm, "FROM"), "{spans:?}");
    assert!(has(&spans, Class::Keyword, "--from=builder"), "{spans:?}");
    assert_eq!(language("dockerfile").outline(text)[0].name, "builder");
    let compose = "services:\n  web:\n    image: node:22\n    environment:\n      PORT: ${PORT}\n";
    let lang = language("docker-compose");
    assert_eq!(lang.outline(compose)[0].name, "web");
    assert_eq!(lang.indent_for("services:", ""), "  ");
    assert!(has(&hl("docker-compose", compose), Class::SpecialForm, "services"));
    let module = manifests().into_iter().find(|m| m.id == "docker").unwrap();
    for action in ["up -d", "stop", "restart", "ps --all", "logs --no-color --tail 200", "down"] { assert!(module.run.iter().any(|r| r.command.ends_with(action)), "missing {action}"); }
    assert!(!module.run.iter().any(|r| r.command.contains("down -v") || r.command.contains("prune")));
    let check = module.check.first().unwrap();
    let re = ide_core::modules::check::compile(&check.pattern).unwrap();
    let errors = ide_core::modules::check::problems(&re, "yaml: line 7: mapping values are not allowed\n", 10);
    assert_eq!(errors[0].line, 7);
    assert!(!ide_core::modules::check::problems(&re, "services.web: invalid type\n", 10).is_empty());
}

/// (class, text) of every span of `text` in the language `id`.
fn hl(id: &str, text: &str) -> Vec<(Class, String)> {
    language(id).highlight(text).iter().map(|s| (s.class, text[s.start..s.end].to_string())).collect()
}

fn has(got: &[(Class, String)], c: Class, t: &str) -> bool {
    got.iter().any(|(k, s)| *k == c && s == t)
}

#[test]
fn the_index_is_current_and_every_module_is_valid() {
    let src = Source::Dir(registry_dir());
    let index: Index = src.fetch_index().expect("index.json");
    let ids: Vec<&str> = index.modules.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["lisp", "c-cpp", "python", "assembler", "html", "css", "markdown", "restructuredtext", "docker", "javascript", "shell-make"]);
    for e in &index.modules {
        assert!(e.files.iter().any(|f| f.name == "module.json"), "{}", e.id);
        for f in &e.files {
            let bytes = src.fetch(&format!("{}/{}", e.path, f.name)).unwrap_or_else(|er| panic!("{}/{}: {er}", e.id, f.name));
            assert_eq!(bytes.len() as u64, f.size, "{}/{} changed: run python tools/build_registry.py", e.id, f.name);
            assert_eq!(sha256_hex(&bytes), f.sha256, "{}/{} changed: run python tools/build_registry.py", e.id, f.name);
        }
        let m = Manifest::parse(&String::from_utf8(src.fetch(&format!("{}/module.json", e.path)).unwrap()).unwrap()).unwrap();
        assert_eq!((m.id.as_str(), m.version.as_str(), m.name.as_str()), (e.id.as_str(), e.version.as_str(), e.name.as_str()));
        assert!(m.cover == e.cover && e.files.iter().any(|f| f.name == m.cover), "{} has no cover", e.id);
        assert!(!m.summary.is_empty() && !m.description.is_empty() && !m.category.is_empty(), "{}", e.id);
        assert!(!m.detect.extensions.is_empty() || !m.detect.files.is_empty(), "{} cannot be detected", e.id);
    }
}

#[test]
fn every_language_is_claimed_by_the_files_it_should() {
    let mut set = LanguageSet::new();
    for m in manifests() {
        for l in m.languages {
            set.add(l).unwrap();
        }
    }
    let id = |p: &str, first: Option<&str>| set.for_path(std::path::Path::new(p), first).id().to_string();
    assert_eq!(id("a/main.c", None), "c");
    assert_eq!(id("include/x.h", None), "c");
    assert_eq!(id("src/x.cpp", None), "cpp");
    assert_eq!(id("tool.py", None), "python");
    assert_eq!(id("boot.asm", None), "nasm");
    assert_eq!(id("start.S", None), "gas");
    assert_eq!(id("README.md", None), "markdown");
    assert_eq!(id("docs/index.rst", None), "rst");
    assert_eq!(id("Makefile", None), "make");
    assert_eq!(id("build.sh", None), "shell");
    assert_eq!(id("run", Some("#!/bin/bash")), "shell");
    assert_eq!(id("docker/Dockerfile", None), "dockerfile");
    assert_eq!(id("docker/Dockerfile.dev", None), "dockerfile");
    assert_eq!(id("Dockerfile.dockerignore", None), "dockerignore");
    assert_eq!(id("compose.yaml", None), "docker-compose");
    assert_eq!(id("compose.production.yaml", None), "docker-compose");
    assert_eq!(id("package.json", None), "javascript-package");
    assert_eq!(id("index.mjs", None), "javascript");
    assert_eq!(id("Component.jsx", None), "jsx");
    assert_eq!(id("link.ld", None), "c");
    assert_eq!(id("notes.xyz", None), "plain");
    assert_eq!(id("web/INDEX.HTML", None), "html");
    assert_eq!(id("page.htm", None), "html");
    assert_eq!(id("page.xhtml", None), "html");
    assert_eq!(id("page", Some("<!DOCTYPE html>")), "html");
    assert_eq!(id("web/styles.CSS", None), "css");
    assert_eq!(id("styles.scss", None), "plain");
}

#[test]
fn html_tags_attributes_and_prose_are_separate() {
    let src = "<!DOCTYPE html>\n<!-- <fake> -->\n<my-card id=hero data-name=\"a > b\"\n aria-label='Привет' disabled>\nText class=plain, \"quotes\" &amp; &#169; &#xA9;\n</my-card>";
    let h = hl("html", src);
    assert!(has(&h, Class::Preproc, "<!DOCTYPE html>") && has(&h, Class::Comment, "<!-- <fake> -->"), "{h:?}");
    assert!(has(&h, Class::SpecialForm, "my-card") && has(&h, Class::Keyword, "data-name"), "{h:?}");
    assert!(has(&h, Class::String, "hero") && has(&h, Class::String, "\"a > b\"") && has(&h, Class::String, "'Привет'"), "{h:?}");
    assert!(has(&h, Class::Keyword, "aria-label") && has(&h, Class::Keyword, "disabled"));
    for entity in ["&amp;", "&#169;", "&#xA9;"] {
        assert!(has(&h, Class::Constant, entity));
    }
    assert!(!h.iter().any(|(_, s)| s.contains("plain") || s.contains("quotes")), "page text is plain: {h:?}");
    let lang = language("html");
    assert_eq!(lang.outline(src).iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["my-card"]);
    let sp = lang.highlight(src);
    assert!(sp.windows(2).all(|w| w[0].end <= w[1].start));
    assert!(has(&hl("html", "<input placeholder=\"unfinished"), Class::String, "\"unfinished"));
    assert!(lang.words().iter().any(|(w, _)| w == "aria-label"));
}

#[test]
fn html_raw_text_and_tag_folding() {
    let lang = language("html");
    let src = "<DIV>\n<script>\nconst x = '<fake>';\n</script>\n<style>\n.x::after { content: '<fake>'; }\n</style>\n<textarea><fake></textarea>\n<img src='x'>\n<my-icon />\n<section>\ntext\n</section>\n</DIV>";
    let spans = lang.highlight(src);
    let names: Vec<String> = lang.outline(src).into_iter().map(|d| d.name).collect();
    assert_eq!(names, ["DIV", "script", "style", "textarea", "img", "my-icon", "section"]);
    assert_eq!(lang.fold_regions(src, &spans), [(0, 13), (1, 3), (4, 6), (10, 12)]);
    assert_eq!(lang.indent_for("<div>", ""), "  ");
    assert_eq!(lang.indent_for("  <img src='x'>", ""), "  ");
    assert_eq!(lang.indent_for("  <my-icon />", ""), "  ");
    assert_eq!(lang.indent_for("  <p>Text</p>", ""), "  ");
    assert_eq!(lang.indent_for("  <!-- <fake> -->", ""), "  ");
    assert_eq!(lang.indent_for("  text", "</div>"), "");
    assert_eq!(lang.indent_for("<div><section>", ""), "    ");
    assert!(lang.split_tag_pair("<DIV title='>'>", "</div>"));
    assert!(!lang.split_tag_pair("<br>", "</br>"));
    assert!(!lang.split_tag_pair("<div>", "</span>"));
    // An unfinished comment or raw-text element must not invent structure.
    assert!(lang.outline("<!--\n<fake>").is_empty());
    assert_eq!(lang.outline("<script>\n<fake>").len(), 1);
}

#[test]
fn css_selectors_declarations_and_brackets() {
    let src = "/* .fake { color: red; } */\n#abc, .card:hover::before {\n  --accent: #abc;\n  background-color: var(--accent);\n  width: calc(100% - 2rem);\n  content: \"}; /* text */\";\n  opacity: .5 !important;\n  background-image: url(data:image/png;base64,abc);\n}\n@media (max-width: 40rem) {\n  .card { padding: 1rem; }\n}\n";
    let h = hl("css", src);
    assert!(has(&h, Class::Type, "#abc") && has(&h, Class::Number, "#abc"), "ID and colour: {h:?}");
    assert!(has(&h, Class::Builtin, ":hover") && has(&h, Class::Builtin, "::before"), "{h:?}");
    assert!(has(&h, Class::Keyword, "background-color") && has(&h, Class::SpecialVar, "--accent"));
    assert!(has(&h, Class::Builtin, "var") && has(&h, Class::Builtin, "calc"));
    for n in ["100%", "2rem", ".5", "40rem", "1rem"] {
        assert!(has(&h, Class::Number, n), "{n}: {h:?}");
    }
    assert!(has(&h, Class::String, "\"}; /* text */\"") && has(&h, Class::String, "url(data:image/png;base64,abc)"), "{h:?}");
    assert!(has(&h, Class::SpecialForm, "!important") && has(&h, Class::Preproc, "@media"));
    let lang = language("css");
    let spans = lang.highlight(src);
    assert!(spans.windows(2).all(|w| w[0].end <= w[1].start));
    assert_eq!(lang.fold_regions(src, &spans), [(1, 8), (9, 11)]);
    let at = src.find("var(").unwrap() + 3;
    assert_eq!(lang.matching_bracket(src, &spans, at), Some((at, at + "(--accent".len())));
    assert_eq!(spans.iter().find(|s| s.start == at).unwrap().depth, 1);
    assert_eq!(lang.outline(src).iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["#abc, .card:hover::before", "@media (max-width: 40rem)", ".card"]);
    assert_eq!(lang.indent_for(".card {", ""), "  ");
    assert_eq!(lang.indent_for("  color: red;", "}"), "");
    assert!(lang.words().iter().any(|(w, _)| w == "background-color"));
}

#[test]
fn web_modules_detect_projects_and_ship_usable_templates() {
    let mods = manifests();
    let rules: Vec<_> = mods.iter().map(|m| (m.id.clone(), m.detect.clone())).collect();
    let paths = [std::path::Path::new("web/index.HTML"), std::path::Path::new("assets/styles.CSS")];
    let (exts, names) = ide_core::modules::detect::scan_files(paths);
    assert_eq!(ide_core::modules::detect::needed_in(&exts, &names, &rules), ["html", "css"]);
    for id in ["html", "css"] {
        let m = mods.iter().find(|m| m.id == id).unwrap();
        for t in &m.templates {
            assert!(t.files.contains_key(&t.open));
            for (path, content) in &t.files {
                let language_id = if path.ends_with(".css") { "css" } else { "html" };
                let l = language(language_id);
                assert!(!l.highlight(content).is_empty());
                assert!(!l.outline(content).is_empty());
            }
        }
    }
    let html = mods.iter().find(|m| m.id == "html").unwrap();
    assert!(html.templates[0].files["index.html"].contains("href=\"styles.css\""));
    assert!(html.templates[0].files.contains_key("styles.css"));
}

#[test]
fn c_and_cpp() {
    let c = hl("c", "#include <stdio.h>\n#define MAX 10\nint main(void) {\n    printf(\"hi %d\\n\", 42); // note\n    return 0x1F;\n}\n");
    assert!(has(&c, Class::Preproc, "#include") && has(&c, Class::String, "<stdio.h>"), "{c:?}");
    assert!(has(&c, Class::Preproc, "#define"));
    assert!(has(&c, Class::Type, "int") && has(&c, Class::Type, "void"));
    assert!(has(&c, Class::Definition, "main") && has(&c, Class::Definition, "printf"), "function names: {c:?}");
    assert!(has(&c, Class::String, "\"hi %d\\n\"") && has(&c, Class::Number, "42") && has(&c, Class::Number, "0x1F"));
    assert!(has(&c, Class::Comment, "// note") && has(&c, Class::SpecialForm, "return"));
    // brackets are still brackets after a function name
    let lang = language("c");
    let src = "f(a, g(b));";
    let sp = lang.highlight(src);
    assert_eq!(sp.iter().filter(|s| s.class == Class::Bracket).map(|s| s.depth).collect::<Vec<_>>(), [0, 1, 1, 0], "{sp:?}");
    let cpp = hl("cpp", "class Foo : public Bar {\n  std::vector<int> v;\n  Foo() noexcept {}\n};\ntemplate <typename T> T id(T x) { return x; }\n");
    assert!(has(&cpp, Class::SpecialForm, "class") && has(&cpp, Class::SpecialForm, "public") && has(&cpp, Class::SpecialForm, "template"), "{cpp:?}");
    assert!(has(&cpp, Class::Type, "vector"));
    // the outline: functions, structs, enums, classes and macros
    let o = lang.outline("#define N 4\nstruct point {\n int x;\n};\nenum color { RED };\nstatic int add(int a, int b)\n{\n return a + b;\n}\nvoid run(void) {\n}\n");
    let names: Vec<&str> = o.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["N", "point", "color", "add", "run"], "{o:?}");
}

#[test]
fn python() {
    let src = "import os\n\n@decorator\nclass Foo(Base):\n    \"\"\"Doc.\"\"\"\n    def method(self, x=1):\n        return f\"{x} # not a comment\"  # a comment\n\nasync def go():\n    print(len([1, 2.5, 0x10]), None)\n";
    let p = hl("python", src);
    assert!(has(&p, Class::SpecialForm, "import") && has(&p, Class::SpecialForm, "return") && has(&p, Class::SpecialForm, "async"), "{p:?}");
    assert!(has(&p, Class::Prefix, "@decorator"));
    assert!(has(&p, Class::Type, "Foo") && has(&p, Class::Definition, "method") && has(&p, Class::Definition, "go"));
    assert!(has(&p, Class::String, "\"\"\"Doc.\"\"\"") && has(&p, Class::String, "f\"{x} # not a comment\""), "{p:?}");
    assert!(has(&p, Class::Comment, "# a comment"));
    assert!(has(&p, Class::SpecialVar, "self") && has(&p, Class::Constant, "None") && has(&p, Class::Builtin, "len") && has(&p, Class::Builtin, "print"));
    assert!(has(&p, Class::Number, "2.5") && has(&p, Class::Number, "0x10"));
    let lang = language("python");
    let names: Vec<(String, usize)> = lang.outline(src).iter().map(|d| (d.name.clone(), d.line)).collect();
    assert_eq!(names, [("Foo".to_string(), 3), ("method".to_string(), 5), ("go".to_string(), 8)]);
    // a def inside a docstring is not an entry
    assert_eq!(lang.outline("def real():\n    '''\n    def fake():\n    '''\n").len(), 1);
    assert_eq!(lang.indent_for("def f(x):", ""), "    ");
}

#[test]
fn assembly() {
    let n = hl("nasm", "section .text\nglobal _start\n_start:\n    mov eax, 0x10 ; load\n    int 0x80\nmsg db \"hi\", 0\n    mov [ebx+4], ax\n");
    assert!(has(&n, Class::Preproc, "section") && has(&n, Class::Preproc, "global") && has(&n, Class::Preproc, "db"), "{n:?}");
    assert!(has(&n, Class::Definition, "_start"), "a label: {n:?}");
    assert!(has(&n, Class::SpecialForm, "mov") && has(&n, Class::SpecialForm, "int"));
    assert!(has(&n, Class::SpecialVar, "eax") && has(&n, Class::SpecialVar, "ebx") && has(&n, Class::SpecialVar, "ax"));
    assert!(has(&n, Class::Number, "0x10") && has(&n, Class::Number, "0x80") && has(&n, Class::Comment, "; load") && has(&n, Class::String, "\"hi\""));
    assert_eq!(language("nasm").outline("_start:\n    mov eax, 1\nloop_a:\n    jmp loop_a\n").len(), 2);
    let g = hl("gas", ".globl _start\n_start:\n    movl $1, %eax\n    int $0x80  # call\n");
    assert!(has(&g, Class::Prefix, ".globl") || has(&g, Class::Preproc, ".globl"), "{g:?}");
    assert!(has(&g, Class::SpecialVar, "%eax") && has(&g, Class::Number, "$1") && has(&g, Class::Comment, "# call"), "{g:?}");
    assert!(has(&g, Class::Definition, "_start") && has(&g, Class::SpecialForm, "movl"));
}

#[test]
fn markdown() {
    let src = "# Title\n\nSome **bold**, *italic* and `code` with a [link](http://x.y) and snake_case_name.\n\n- item\n  - nested\n1. one\n> quoted\n\n```sh\n# not a heading\necho hi\n```\n\nSetext\n------\n";
    let m = hl("markdown", src);
    assert!(has(&m, Class::Heading, "# Title"), "{m:?}");
    assert!(has(&m, Class::Strong, "**bold**") && has(&m, Class::Emphasis, "*italic*") && has(&m, Class::Code, "`code`"));
    assert!(has(&m, Class::Link, "[link](http://x.y)"));
    assert!(!m.iter().any(|(k, s)| *k == Class::Emphasis && s.contains("case")), "snake_case is not emphasis: {m:?}");
    assert!(has(&m, Class::Quote, "> quoted"));
    assert!(m.iter().any(|(k, s)| *k == Class::Code && s.starts_with("```sh") && s.contains("# not a heading") && s.trim_end().ends_with("```")), "a fenced block: {m:?}");
    assert!(m.iter().any(|(k, s)| *k == Class::Heading && s.starts_with("Setext")));
    let lang = language("markdown");
    let o: Vec<(String, String)> = lang.outline(src).iter().map(|d| (d.name.clone(), d.operator.clone())).collect();
    assert_eq!(o, [("Title".to_string(), "h1".to_string()), ("Setext".to_string(), "h2".to_string())], "the # inside the code block is not a heading");
    assert_eq!(lang.continue_line("- item").unwrap().insert, "- ");
    assert_eq!(lang.fold_regions(src, &[])[0].0, 0);
}

#[test]
fn restructuredtext() {
    let src = "Title\n=====\n\nSection\n-------\n\n.. note::\n\n   Text with :ref:`target`, ``literal`` and *emphasis*.\n\n.. _target: http://x.y\n.. just a comment\n\nExample::\n\n    code\n\n- item\n";
    let r = hl("rst", src);
    assert!(r.iter().any(|(k, s)| *k == Class::Heading && s.starts_with("Title\n====")), "{r:?}");
    assert!(has(&r, Class::Preproc, "..") && has(&r, Class::SpecialForm, "note"), "{r:?}");
    assert!(has(&r, Class::SpecialForm, ":ref:") && has(&r, Class::Link, "`target`"));
    assert!(has(&r, Class::Code, "``literal``") && has(&r, Class::Emphasis, "*emphasis*"));
    assert!(r.iter().any(|(k, s)| *k == Class::Link && s.starts_with(".. _target")));
    assert!(r.iter().any(|(k, s)| *k == Class::Comment && s == ".. just a comment"), "a plain `..` line is a comment: {r:?}");
    let lang = language("rst");
    let o: Vec<(String, String)> = lang.outline(src).iter().map(|d| (d.name.clone(), d.operator.clone())).collect();
    assert_eq!(o, [("Title".to_string(), "h1".to_string()), ("Section".to_string(), "h2".to_string())]);
}

#[test]
fn shell_make_and_config() {
    let sh = hl("shell", "#!/bin/sh\n# comment\necho $USER $1\nbuild() {\n  if [ -z x ]; then echo 'none'; fi\n}\nls ${HOME} # trailing\n");
    assert!(has(&sh, Class::Preproc, "#!/bin/sh") && has(&sh, Class::Comment, "# comment") && has(&sh, Class::Comment, "# trailing"), "{sh:?}");
    assert!(has(&sh, Class::SpecialVar, "$USER") && has(&sh, Class::SpecialVar, "${HOME}") && has(&sh, Class::SpecialVar, "$1"));
    assert!(has(&sh, Class::SpecialForm, "if") && has(&sh, Class::SpecialForm, "then") && has(&sh, Class::Builtin, "echo") && has(&sh, Class::String, "'none'"));
    assert!(has(&sh, Class::Definition, "build"));
    assert_eq!(language("shell").outline("a() {\n}\nfunction b {\n}\nc () {\n}\n").iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["a", "c"]);

    let mk = "CC := gcc\nOBJS = a.o b.o\n.PHONY: all clean\nall: bin/kernel # default\nbin/kernel: $(OBJS)\n\t@$(CC) -o $@ $^\nifeq ($(X),1)\nendif\n";
    let m = hl("make", mk);
    assert!(has(&m, Class::SpecialVar, "CC") && has(&m, Class::SpecialVar, "OBJS"), "{m:?}");
    assert!(has(&m, Class::Preproc, ".PHONY") && has(&m, Class::Preproc, "ifeq"));
    assert!(has(&m, Class::Definition, "all") && has(&m, Class::Definition, "bin/kernel"), "targets: {m:?}");
    assert!(has(&m, Class::SpecialVar, "$(OBJS)") && has(&m, Class::SpecialVar, "$@") && has(&m, Class::SpecialVar, "$^") && has(&m, Class::Prefix, "\t@"));
    assert!(has(&m, Class::Comment, "# default"));
    assert_eq!(language("make").outline(mk).iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["all", "bin/kernel"], "assignments are not targets");

    let y = hl("config", "# c\nname: demo\nlist:\n  - port: 8080\n    on: true\n[section]\nkey = \"v\"\n");
    assert!(has(&y, Class::Comment, "# c") && has(&y, Class::SpecialVar, "name") && has(&y, Class::SpecialVar, "port") && has(&y, Class::Number, "8080"), "{y:?}");
    assert!(has(&y, Class::Constant, "true") && has(&y, Class::Heading, "[section]") && has(&y, Class::String, "\"v\""));
}

#[test]
fn every_run_command_names_a_language_and_a_known_placeholder() {
    for m in manifests() {
        for r in &m.run {
            for cmd in [&r.command, &r.windows] {
                let stripped = cmd.replace("{file}", "").replace("{dir}", "").replace("{name}", "").replace("{stem}", "").replace("{image}", "");
                assert!(!stripped.contains('{') && !stripped.contains('}'), "{}: {cmd}", m.id);
            }
            assert!(!r.command.is_empty() && !r.name.is_empty());
        }
        for t in &m.tools {
            assert!(!t.programs.is_empty() && !t.apt.is_empty(), "{}: {}", m.id, t.name);
        }
        for t in &m.templates {
            assert!(!t.files.is_empty(), "{}: {}", m.id, t.name);
            for p in t.files.keys() {
                assert!(!p.starts_with('/') && !p.contains(".."), "{}: {p}", m.id);
            }
        }
    }
}

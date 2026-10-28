//! The language server client against the npm servers the modules install (typescript-language-server, basedpyright,
//! the Dockerfile and Compose servers): what the client reads, checked against what each server says. Runs only when
//! they are installed, the way Light installs them:
//!
//! - `LIGHT_TEST_NODE`: the folder of a Node.js runtime (`node` or `node.exe` in it, or in its `bin`);
//! - `LIGHT_TEST_SERVERS`: a folder with one folder per server (`typescript-language-server`, `basedpyright`,
//!   `dockerfile-language-server`, `compose-language-service`), each with its `node_modules` from `npm ci`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ide_core::lsp::proto;
use ide_core::lsp::{Client, LineMap, Output, Session, uri};
use serde_json::{Value, json};

struct Server {
    session: Session,
    root: PathBuf,
}

/// The server `bin` of the npm install `name`, or `None` (and the test is skipped) when nothing is installed.
fn start(name: &str, bin: &str, args: &[&str], root: &Path, options: Value) -> Option<Server> {
    let (Ok(node), Ok(servers)) = (std::env::var("LIGHT_TEST_NODE"), std::env::var("LIGHT_TEST_SERVERS")) else {
        eprintln!("LIGHT_TEST_NODE / LIGHT_TEST_SERVERS not set: skipped");
        return None;
    };
    let node = PathBuf::from(node);
    let bins = Path::new(&servers).join(name).join("node_modules").join(".bin");
    let path = std::env::join_paths([node.join("bin"), node.clone(), bins.clone()].into_iter().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/D", "/C"]).arg(bins.join(format!("{bin}.cmd"))).args(args);
        c
    } else {
        let mut c = Command::new(bins.join(bin));
        c.args(args);
        c
    };
    cmd.env("PATH", path).current_dir(root);
    let client = Client::spawn(cmd, None).expect("the server starts");
    let session = Session::start(client, &root.to_string_lossy().replace('\\', "/"), options);
    let mut s = Server { session, root: root.to_path_buf() };
    s.until("readiness", |o| matches!(o, Output::Ready).then_some(()));
    Some(s)
}

impl Server {
    fn uri(&self, rel: &str) -> String {
        uri::from_path(&self.root.join(rel).to_string_lossy().replace('\\', "/"))
    }

    fn until<T>(&mut self, what: &str, mut f: impl FnMut(Output) -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            for o in self.session.poll() {
                if let Output::Exited { .. } = o {
                    panic!("the server exited: {:?}", self.session.client().log());
                }
                if let Some(t) = f(o) {
                    return t;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(90), "no {what}: {:?}", self.session.client().log());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn ask(&mut self, method: &str, params: Value) -> Value {
        let id = self.session.request(method, params).expect("ready");
        self.until(method, |o| match o {
            Output::Response { id: got, result } if got == id => Some(result.unwrap_or_else(|e| panic!("{method}: {}", e.message))),
            _ => None,
        })
    }

    fn open(&mut self, rel: &str, lang: &str) -> (String, String) {
        let text = std::fs::read_to_string(self.root.join(rel)).unwrap();
        let u = self.uri(rel);
        self.session.open(&u, lang, &text);
        (u, text)
    }

    /// The problems published for `uri` that satisfy `want` (servers may publish an empty list first).
    fn problems(&mut self, u: &str, want: impl Fn(&[proto::Diagnostic]) -> bool) -> Vec<proto::Diagnostic> {
        self.until("diagnostics", |o| match o {
            Output::Diagnostics { uri, list, .. } if uri == u && want(&list) => Some(list),
            _ => None,
        })
    }

    /// `textDocument` and `position` of the `nth` occurrence of `needle` (one character into it).
    fn at(&self, u: &str, text: &str, needle: &str, nth: usize) -> Value {
        let off = text.match_indices(needle).nth(nth).unwrap_or_else(|| panic!("{needle:?} in {text:?}")).0 + 1;
        json!({"textDocument": {"uri": u}, "position": LineMap::new(text).position(off, self.session.caps.encoding)})
    }

    fn stop(mut self) {
        self.session.stop(Duration::from_secs(5));
        self.session.client_mut().kill();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn project(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!("light-lsp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    root
}

/// The file names of the places of an answer.
fn places(v: &Value) -> Vec<String> {
    proto::locations(v).into_iter().map(|l| l.uri.rsplit('/').next().unwrap_or("").to_string()).collect()
}

/// What the JavaScript module gives typescript-language-server: a rename renames the name everywhere, not by an
/// alias in the importing file (`import { add as sum }`).
fn ts_options() -> Value {
    json!({"preferences": {"providePrefixAndSuffixTextForRename": false}})
}

#[test]
fn typescript_language_server() {
    let root = project("ts", &[
        ("util.js", "/** Adds two numbers. @param {number} a @param {number} b */\nexport function add(a, b) { return a + b; }\n"),
        ("main.js", "// @ts-check\nimport { add } from \"./util.js\";\nconst s = \"привет\"; const total = add(1, 2);\nconst n = Math.ma;\nconst bad = add(\"x\", 2);\n"),
    ]);
    let Some(mut s) = start("typescript-language-server", "typescript-language-server", &["--stdio"], &root, ts_options()) else { return };
    assert!(s.session.caps.definition && s.session.caps.references && s.session.caps.rename && s.session.caps.completion && s.session.caps.hover && s.session.caps.formatting);
    let (main, text) = s.open("main.js", "javascript");
    // a type error the checker finds in JavaScript with // @ts-check
    let problems = s.problems(&main, |l| !l.is_empty());
    assert!(problems.iter().any(|d| d.message.contains("string")), "{problems:?}");
    // after letters that are 2 bytes in UTF-8 and 1 unit in UTF-16
    assert_eq!(places(&s.ask("textDocument/definition", s.at(&main, &text, "add(1", 0))), ["util.js"]);
    let hover = proto::hover_parts(&proto::hover_text(&s.ask("textDocument/hover", s.at(&main, &text, "add(1", 0))).unwrap());
    assert!(hover.signature.contains("add(a: number, b: number): number"), "{hover:?}");
    assert!(hover.doc.contains("Adds two numbers"), "{hover:?}");
    let mut refs = s.at(&main, &text, "add(1", 0);
    refs["context"] = json!({"includeDeclaration": true});
    let found = places(&s.ask("textDocument/references", refs));
    assert!(found.iter().filter(|f| *f == "main.js").count() >= 3 && found.iter().any(|f| f == "util.js"), "{found:?}");
    let mut pos = s.at(&main, &text, "Math.ma", 0);
    pos["position"]["character"] = json!(pos["position"]["character"].as_u64().unwrap() + 6);
    let (items, _) = proto::completion(&s.ask("textDocument/completion", pos));
    assert!(items.iter().any(|i| i.insert == "max"), "{:?}", items.iter().take(10).collect::<Vec<_>>());
    let mut rename = s.at(&main, &text, "add(1", 0);
    rename["newName"] = json!("sum");
    let edit = proto::workspace_edit(&s.ask("textDocument/rename", rename)).unwrap();
    let files: Vec<&str> = edit.iter().map(|(u, _)| u.rsplit('/').next().unwrap()).collect();
    assert!(files.contains(&"main.js") && files.contains(&"util.js"), "{files:?}");
    s.session.change(&main, "const  a=1;function  f( ){return a}\n");
    let formatted = proto::text_edits(&s.ask("textDocument/formatting", json!({"textDocument": {"uri": main}, "options": {"tabSize": 4, "insertSpaces": true}})));
    let pretty = proto::apply(s.session.text(&main).unwrap(), &formatted, s.session.caps.encoding).unwrap();
    assert!(pretty.contains("const a = 1;") && pretty.contains("function f() {"), "{pretty}");
    s.stop();
}

#[test]
fn basedpyright() {
    let root = project("py", &[
        ("util.py", "def add(a: int, b: int) -> int:\n    \"\"\"Adds two numbers.\"\"\"\n    return a + b\n"),
        ("main.py", "from util import add\n\ns = \"привет\"; total = add(1, 2)\nprint(undefined_name)\nimport math\nmath.sq\n"),
    ]);
    let Some(mut s) = start("basedpyright", "basedpyright-langserver", &["--stdio"], &root, Value::Null) else { return };
    assert!(s.session.caps.definition && s.session.caps.references && s.session.caps.rename && s.session.caps.completion && s.session.caps.hover);
    let (main, text) = s.open("main.py", "python");
    let problems = s.problems(&main, |l| l.iter().any(|d| d.message.contains("undefined_name")));
    assert!(problems.iter().any(|d| d.severity == 1 && d.message.contains("undefined_name")), "{problems:?}");
    assert_eq!(places(&s.ask("textDocument/definition", s.at(&main, &text, "add(1", 0))), ["util.py"]);
    let hover = proto::hover_parts(&proto::hover_text(&s.ask("textDocument/hover", s.at(&main, &text, "add(1", 0))).unwrap());
    assert_eq!(hover.kind, "function");
    assert!(hover.signature.starts_with("def add(") && hover.signature.contains("b: int") && hover.signature.ends_with("-> int"), "{hover:?}");
    assert!(hover.doc.contains("Adds two numbers"), "{hover:?}");
    let mut pos = s.at(&main, &text, "math.sq", 0);
    pos["position"]["character"] = json!(pos["position"]["character"].as_u64().unwrap() + 6);
    let (items, _) = proto::completion(&s.ask("textDocument/completion", pos));
    assert!(items.iter().any(|i| i.insert == "sqrt"), "{:?}", items.iter().take(10).collect::<Vec<_>>());
    let mut rename = s.at(&main, &text, "add(1", 0);
    rename["newName"] = json!("total_of");
    let edit = proto::workspace_edit(&s.ask("textDocument/rename", rename)).unwrap();
    let files: Vec<&str> = edit.iter().map(|(u, _)| u.rsplit('/').next().unwrap()).collect();
    assert!(files.contains(&"main.py") && files.contains(&"util.py"), "{files:?}");
    s.stop();
}

#[test]
fn dockerfile_language_server() {
    let root = project("docker", &[("Dockerfile", "FROM alpine:3.20\nRUNN echo hi\nCMD [\"echo\", \"ok\"]\n")]);
    let Some(mut s) = start("dockerfile-language-server", "docker-langserver", &["--stdio"], &root, Value::Null) else { return };
    let (u, text) = s.open("Dockerfile", "dockerfile");
    let problems = s.problems(&u, |l| !l.is_empty());
    assert!(problems.iter().any(|d| d.message.to_lowercase().contains("unknown instruction")), "{problems:?}");
    let hover = proto::hover_text(&s.ask("textDocument/hover", s.at(&u, &text, "FROM", 0))).unwrap();
    assert!(hover.contains("baseImage"), "{hover}");
    let (items, _) = proto::completion(&s.ask("textDocument/completion", json!({"textDocument": {"uri": u}, "position": {"line": 2, "character": 0}})));
    assert!(items.iter().any(|i| i.label == "RUN"), "{:?}", items.iter().take(10).collect::<Vec<_>>());
    s.stop();
}

#[test]
fn compose_language_service() {
    let root = project("compose", &[("compose.yaml", "services:\n  web:\n    image: alpine:3.20\n    command: [echo, ok]\n")]);
    let Some(mut s) = start("compose-language-service", "docker-compose-langserver", &["--stdio"], &root, Value::Null) else { return };
    let (u, text) = s.open("compose.yaml", "dockercompose");
    let hover = proto::hover_text(&s.ask("textDocument/hover", s.at(&u, &text, "image", 0))).unwrap_or_default();
    assert!(!hover.is_empty(), "no hover on `image`");
    // broken YAML is a problem
    s.session.change(&u, "services:\n  web: [\n");
    let problems = s.problems(&u, |l| !l.is_empty());
    assert!(problems.iter().any(|d| d.severity == 1), "{problems:?}");
    s.stop();
}

//! The language server client against a real clangd: what the unit tests assume about answers, checked against what
//! a server actually says. Runs only when a clangd is named:
//!
//! - `LIGHT_TEST_CLANGD=clangd` (or a path): a clangd of this system;
//! - `LIGHT_TEST_CLANGD_WSL=Ubuntu-24.04`: the clangd of that WSL distribution, on files under `/mnt/<drive>`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ide_core::lsp::proto;
use ide_core::lsp::{Client, Encoding, LineMap, Output, Session, uri};
use serde_json::{Value, json};

/// A path of this system as the server sees it.
type ToServer = Box<dyn Fn(&Path) -> String>;

struct Server {
    session: Session,
    to_server: ToServer,
}

fn start(root: &Path) -> Option<Server> {
    let (cmd, to_server): (Command, ToServer) = if let Ok(exe) = std::env::var("LIGHT_TEST_CLANGD") {
        let mut c = Command::new(exe);
        c.arg("--log=error");
        (c, Box::new(|p: &Path| p.to_string_lossy().replace('\\', "/")))
    } else if let Ok(distro) = std::env::var("LIGHT_TEST_CLANGD_WSL") {
        let mut c = Command::new("wsl.exe");
        c.args(["-d", &distro, "-e", "clangd", "--log=error"]).env("WSL_UTF8", "1");
        let map = |p: &Path| {
            let s = p.to_string_lossy().replace('\\', "/");
            let drive = s[..1].to_ascii_lowercase();
            format!("/mnt/{drive}{}", &s[2..])
        };
        (c, Box::new(map))
    } else {
        eprintln!("no clangd named (LIGHT_TEST_CLANGD / LIGHT_TEST_CLANGD_WSL): skipped");
        return None;
    };
    let client = Client::spawn(cmd, None).expect("clangd starts");
    let session = Session::start(client, &to_server(root), Value::Null);
    Some(Server { session, to_server })
}

impl Server {
    fn uri(&self, p: &Path) -> String {
        uri::from_path(&(self.to_server)(p))
    }

    fn until<T>(&mut self, what: &str, mut f: impl FnMut(Output) -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            for o in self.session.poll() {
                if let Output::Exited { .. } = o {
                    panic!("clangd exited: {:?}", self.session.client().log());
                }
                if let Some(t) = f(o) {
                    return t;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(60), "no {what} from clangd");
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
}

fn project() -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!("light-lsp-clangd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // a string with letters that are two UTF-8 bytes but one UTF-16 unit, before the names the requests point at
    std::fs::write(root.join("util.h"), "#ifndef UTIL_H\n#define UTIL_H\n/* Adds two numbers. */\nint add(int a, int b);\n#endif\n").unwrap();
    std::fs::write(root.join("util.c"), "#include \"util.h\"\nint add(int a, int b) { return a + b; }\n").unwrap();
    std::fs::write(root.join("main.c"), "#include \"util.h\"\nconst char *s = \"привет\"; int x = add(1, 2);\nint main(void) {\n  return add(x, missing);\n}\n").unwrap();
    std::fs::write(root.join("compile_flags.txt"), "-xc\n-std=c11\n").unwrap();
    root
}

#[test]
fn clangd_answers_what_the_client_reads() {
    let root = project();
    let Some(mut s) = start(&root) else { return };
    s.until("readiness", |o| matches!(o, Output::Ready).then_some(()));
    // clangd counts in UTF-8 when asked to
    assert_eq!(s.session.caps.encoding, Encoding::Utf8);
    assert!(s.session.caps.definition && s.session.caps.references && s.session.caps.rename && s.session.caps.completion && s.session.caps.hover && s.session.caps.formatting);
    let enc = s.session.caps.encoding;

    let main = root.join("main.c");
    let main_uri = s.uri(&main);
    let text = std::fs::read_to_string(&main).unwrap();
    s.session.open(&main_uri, "c", &text);
    let map = LineMap::new(&text);
    let at = |needle: &str, nth: usize| -> Value {
        let off = text.match_indices(needle).nth(nth).unwrap().0 + 1;
        serde_json::to_value(map.position(off, enc)).unwrap()
    };

    // the undeclared name is a problem, in the right place
    let (version, problems) = s.until("diagnostics", |o| match o {
        Output::Diagnostics { uri, version, list } if uri == main_uri => Some((version, list)),
        _ => None,
    });
    assert_eq!(version, Some(1));
    let missing = problems.iter().find(|d| d.message.contains("missing")).unwrap_or_else(|| panic!("{problems:?}"));
    assert_eq!(missing.severity, 1);
    let (a, b) = map.range(missing.range, enc);
    assert_eq!(&text[a..b], "missing");

    // `add` after the Cyrillic string: its definition is in util.c (the declaration in util.h)
    let doc = json!({"uri": main_uri});
    let defs = proto::locations(&s.ask("textDocument/definition", json!({"textDocument": doc, "position": at("add(1", 0)})));
    assert!(!defs.is_empty());
    assert!(defs.iter().all(|l| l.uri.ends_with("util.h") || l.uri.ends_with("util.c")), "{defs:?}");

    // the uses in this file (the other files are not indexed without a compile_commands.json)
    let refs = proto::locations(&s.ask("textDocument/references", json!({"textDocument": doc, "position": at("add(x", 0), "context": {"includeDeclaration": true}})));
    let here: Vec<_> = refs.iter().filter(|l| l.uri == main_uri).map(|l| map.range(l.range, enc)).collect();
    assert_eq!(here.len(), 2, "{refs:?}");
    assert!(here.iter().all(|&(a, b)| &text[a..b] == "add"));

    // what is said about it
    let hover = proto::hover_parts(&proto::hover_text(&s.ask("textDocument/hover", json!({"textDocument": doc, "position": at("add(1", 0)}))).unwrap());
    assert_eq!(hover.kind, "function");
    assert!(hover.signature.contains("int add(int a, int b)"), "{hover:?}");
    assert!(hover.doc.contains("Adds two numbers"), "{hover:?}");

    // what may follow `ad`
    let completion = s.ask("textDocument/completion", json!({"textDocument": doc, "position": map.position(text.find("add(x").unwrap() + 2, enc)}));
    let (items, _) = proto::completion(&completion);
    assert!(items.iter().any(|i| i.insert == "add"), "{items:?}");

    // a rename: the edits come for this file (and whatever else clangd knows)
    let edit = s.ask("textDocument/rename", json!({"textDocument": doc, "position": at("add(x", 0), "newName": "sum"}));
    let changes = proto::workspace_edit(&edit).unwrap();
    let (_, edits) = changes.iter().find(|(u, _)| *u == main_uri).unwrap();
    let renamed = proto::apply(&text, edits, enc).unwrap();
    assert!(renamed.contains("int x = sum(1, 2);") && renamed.contains("return sum(x, missing);"), "{renamed}");

    // reformatting keeps the code and fixes the layout
    s.session.change(&main_uri, "int  main( void ){return 0;}\n");
    let formatted = proto::text_edits(&s.ask("textDocument/formatting", json!({"textDocument": doc, "options": {"tabSize": 4, "insertSpaces": true}})));
    let pretty = proto::apply(s.session.text(&main_uri).unwrap(), &formatted, enc).unwrap();
    assert!(pretty.starts_with("int main(void) {"), "{pretty}");

    s.session.close(&main_uri);
    s.session.stop(Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&root);
}

//! One conversation with a server: `initialize` and what the server says it can do, the documents it is told about
//! (always their whole text: every server takes that), requests, and `shutdown` / `exit` at the end.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::client::{Client, Event, ResponseError};
use super::pos::Encoding;
use super::proto::{self, Diagnostic};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// `initialize` was sent and is not answered yet.
    Starting,
    Ready,
    /// `shutdown` was sent.
    Stopping,
    Exited,
}

/// What the server said it can do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub encoding: Encoding,
    /// It wants to hear about open documents and their changes.
    pub sync: bool,
    /// It wants `didSave`.
    pub save: bool,
    pub completion: bool,
    /// Typing one of these asks for completions at once (`.`, `->`...).
    pub completion_triggers: Vec<String>,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub rename: bool,
    pub formatting: bool,
}

/// A provider is on when it is `true` or an object of options.
fn provides(v: &Value) -> bool {
    v.as_bool().unwrap_or(v.is_object())
}

impl Capabilities {
    /// From the `capabilities` of the answer to `initialize`.
    pub fn from_json(c: &Value) -> Capabilities {
        let sync = &c["textDocumentSync"];
        let (open, change, save) = match sync {
            Value::Number(n) => (n.as_u64() != Some(0), n.as_u64().unwrap_or(0) != 0, false),
            Value::Object(_) => (sync["openClose"].as_bool().unwrap_or(false), sync["change"].as_u64().unwrap_or(0) != 0, provides(&sync["save"])),
            _ => (false, false, false),
        };
        Capabilities {
            encoding: c["positionEncoding"].as_str().map_or(Encoding::Utf16, Encoding::from_name),
            sync: open || change,
            save,
            completion: provides(&c["completionProvider"]),
            completion_triggers: c["completionProvider"]["triggerCharacters"].as_array().map_or_else(Vec::new, |a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()),
            hover: provides(&c["hoverProvider"]),
            definition: provides(&c["definitionProvider"]),
            references: provides(&c["referencesProvider"]),
            rename: provides(&c["renameProvider"]),
            formatting: provides(&c["documentFormattingProvider"]),
        }
    }
}

/// What came of [`Session::poll`].
#[derive(Debug)]
pub enum Output {
    /// The server is ready: documents are sent to it and it takes requests.
    Ready,
    Response { id: u64, result: Result<Value, ResponseError> },
    /// The problems the server found in a document, for the version of its text it says (if it does).
    Diagnostics { uri: String, version: Option<i64>, list: Vec<Diagnostic> },
    /// A message the server wants the user to read (an error or a warning).
    Message { error: bool, text: String },
    /// `initialize` failed.
    Failed(String),
    /// The server is gone; `expected` when it was asked to stop.
    Exited { expected: bool },
}

struct Doc {
    language: String,
    version: i64,
    text: String,
    /// `didOpen` went out (it waits until the server is ready).
    sent: bool,
}

pub struct Session {
    client: Client,
    pub state: State,
    pub caps: Capabilities,
    init_id: u64,
    shutdown_id: Option<u64>,
    docs: HashMap<String, Doc>,
}

/// What the IDE can do with what a server says.
fn client_capabilities() -> Value {
    json!({
        "general": {"positionEncodings": ["utf-8", "utf-16"]},
        // the same, as clangd asked for it before the protocol had it
        "offsetEncoding": ["utf-8", "utf-16"],
        "textDocument": {
            "synchronization": {"didSave": true, "willSave": false, "willSaveWaitUntil": false, "dynamicRegistration": false},
            "completion": {"completionItem": {"snippetSupport": false, "insertReplaceSupport": false, "documentationFormat": ["plaintext"]}, "contextSupport": false},
            "hover": {"contentFormat": ["markdown", "plaintext"]},
            "definition": {"linkSupport": true},
            "references": {},
            "rename": {"prepareSupport": false},
            "formatting": {},
            "publishDiagnostics": {"relatedInformation": false, "versionSupport": true},
        },
        "workspace": {"workspaceFolders": true, "configuration": false, "applyEdit": false, "didChangeWatchedFiles": {"dynamicRegistration": false}},
        "window": {"workDoneProgress": false},
    })
}

impl Session {
    /// Start the conversation: `root` is the project's folder as the server sees it (`/home/me/demo`, `E:/demo`),
    /// `options` the module's `initializationOptions`.
    pub fn start(mut client: Client, root: &str, options: Value) -> Session {
        let uri = super::uri::from_path(root);
        let name = root.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("project");
        let params = json!({
            // the IDE may run on Windows and the server in Linux: no process id the server could watch
            "processId": null,
            "clientInfo": {"name": "Light", "version": env!("CARGO_PKG_VERSION")},
            "rootPath": root,
            "rootUri": uri,
            "workspaceFolders": [{"uri": uri, "name": name}],
            "initializationOptions": if options.is_null() { Value::Null } else { options },
            "capabilities": client_capabilities(),
        });
        let init_id = client.request("initialize", params);
        Session { client, state: State::Starting, caps: Capabilities::default(), init_id, shutdown_id: None, docs: HashMap::new() }
    }

    pub fn ready(&self) -> bool {
        self.state == State::Ready
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut Client {
        &mut self.client
    }

    fn send_open(&mut self, uri: &str) {
        let Some(d) = self.docs.get_mut(uri) else { return };
        d.sent = true;
        let params = json!({"textDocument": {"uri": uri, "languageId": d.language, "version": d.version, "text": d.text}});
        if self.caps.sync {
            self.client.notify("textDocument/didOpen", params);
        }
    }

    /// What happened since the last call.
    pub fn poll(&mut self) -> Vec<Output> {
        let mut out = vec![];
        while let Some(e) = self.client.try_event() {
            match e {
                Event::Response { id, result } if id == self.init_id => match result {
                    Ok(v) => {
                        self.caps = Capabilities::from_json(&v["capabilities"]);
                        if let Some(e) = v["offsetEncoding"].as_str() {
                            self.caps.encoding = Encoding::from_name(e);
                        }
                        self.client.notify("initialized", json!({}));
                        self.state = State::Ready;
                        let waiting: Vec<String> = self.docs.iter().filter(|(_, d)| !d.sent).map(|(u, _)| u.clone()).collect();
                        for uri in waiting {
                            self.send_open(&uri);
                        }
                        out.push(Output::Ready);
                    }
                    Err(e) => {
                        self.state = State::Exited;
                        self.client.kill();
                        out.push(Output::Failed(e.message));
                    }
                },
                Event::Response { id, .. } if Some(id) == self.shutdown_id => self.client.notify("exit", Value::Null),
                Event::Response { id, result } => out.push(Output::Response { id, result }),
                Event::Notification { method, params } => match method.as_str() {
                    "textDocument/publishDiagnostics" => {
                        if let Some((uri, version, list)) = proto::diagnostics(&params) {
                            out.push(Output::Diagnostics { uri, version, list });
                        }
                    }
                    "window/showMessage" => {
                        let kind = params["type"].as_u64().unwrap_or(4);
                        if kind <= 2 {
                            out.push(Output::Message { error: kind == 1, text: params["message"].as_str().unwrap_or("").to_string() });
                        }
                    }
                    _ => {}
                },
                Event::Exited => {
                    let expected = self.state == State::Stopping;
                    if self.state != State::Exited {
                        self.state = State::Exited;
                        out.push(Output::Exited { expected });
                    }
                }
            }
        }
        out
    }

    /// Send a request; `None` while the server is not ready for it.
    pub fn request(&mut self, method: &str, params: Value) -> Option<u64> {
        self.ready().then(|| self.client.request(method, params))
    }

    /// Tell the server about a document (now, or as soon as it is ready).
    pub fn open(&mut self, uri: &str, language: &str, text: &str) {
        if self.docs.contains_key(uri) {
            self.change(uri, text);
            return;
        }
        self.docs.insert(uri.to_string(), Doc { language: language.to_string(), version: 1, text: text.to_string(), sent: false });
        if self.ready() {
            self.send_open(uri);
        }
    }

    /// The document's text is now `text`.
    pub fn change(&mut self, uri: &str, text: &str) {
        let ready = self.ready();
        let Some(d) = self.docs.get_mut(uri) else { return };
        if d.text == text {
            return;
        }
        d.version += 1;
        d.text = text.to_string();
        if ready && d.sent && self.caps.sync {
            let params = json!({"textDocument": {"uri": uri, "version": d.version}, "contentChanges": [{"text": d.text}]});
            self.client.notify("textDocument/didChange", params);
        }
    }

    pub fn save(&mut self, uri: &str) {
        if self.ready() && self.caps.save && self.docs.get(uri).is_some_and(|d| d.sent) {
            self.client.notify("textDocument/didSave", json!({"textDocument": {"uri": uri}}));
        }
    }

    pub fn close(&mut self, uri: &str) {
        if let Some(d) = self.docs.remove(uri)
            && d.sent
            && self.ready()
            && self.caps.sync
        {
            self.client.notify("textDocument/didClose", json!({"textDocument": {"uri": uri}}));
        }
    }

    /// Files that are not open changed on disk (an edit the IDE made to them): the server reads them again.
    pub fn files_changed(&mut self, uris: &[String]) {
        if self.ready() && !uris.is_empty() {
            let changes: Vec<Value> = uris.iter().map(|u| json!({"uri": u, "type": 2})).collect();
            self.client.notify("workspace/didChangeWatchedFiles", json!({"changes": changes}));
        }
    }

    pub fn is_open(&self, uri: &str) -> bool {
        self.docs.contains_key(uri)
    }

    pub fn open_uris(&self) -> Vec<String> {
        self.docs.keys().cloned().collect()
    }

    /// The version of the document's text the server has last been told about.
    pub fn version(&self, uri: &str) -> Option<i64> {
        self.docs.get(uri).map(|d| d.version)
    }

    /// The text the server has of the document (the positions it names are in it).
    pub fn text(&self, uri: &str) -> Option<&str> {
        self.docs.get(uri).map(|d| d.text.as_str())
    }

    /// Ask the server to stop and wait for it, at most `timeout`; what has not ended by then is ended.
    pub fn stop(&mut self, timeout: Duration) {
        let start = Instant::now();
        if self.ready() {
            self.shutdown_id = Some(self.client.request("shutdown", Value::Null));
            self.state = State::Stopping;
            while start.elapsed() < timeout && self.state == State::Stopping && !self.client.exited() {
                self.poll();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.state = State::Exited;
        self.client.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::rpc;
    use std::io::{self, BufReader};

    fn wait(s: &mut Session, what: impl Fn(&Output) -> bool) -> Output {
        let start = Instant::now();
        loop {
            for o in s.poll() {
                if what(&o) {
                    return o;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn capabilities_in_their_several_shapes() {
        let c = Capabilities::from_json(&json!({"positionEncoding": "utf-8", "textDocumentSync": 2, "completionProvider": {"triggerCharacters": [".", "->"]}, "hoverProvider": true, "renameProvider": {"prepareProvider": true}, "referencesProvider": false}));
        assert_eq!(c.encoding, Encoding::Utf8);
        assert!(c.sync && !c.save && c.completion && c.hover && c.rename && !c.references && !c.definition && !c.formatting);
        assert_eq!(c.completion_triggers, vec![".", "->"]);
        let c = Capabilities::from_json(&json!({"textDocumentSync": {"openClose": true, "change": 1, "save": {"includeText": false}}}));
        assert!(c.sync && c.save);
        assert_eq!(c.encoding, Encoding::Utf16);
        assert!(!Capabilities::from_json(&json!({"textDocumentSync": 0})).sync);
    }

    /// A server that does what the conversation below expects, step by step.
    #[test]
    fn a_whole_conversation() {
        let (to_server_r, to_server_w) = io::pipe().unwrap();
        let (to_client_r, mut w) = io::pipe().unwrap();
        let server = std::thread::spawn(move || {
            let mut r = BufReader::new(to_server_r);
            let mut next = || rpc::read_message(&mut r).unwrap().unwrap();
            let init = next();
            assert_eq!(init["method"], "initialize");
            assert_eq!(init["params"]["rootUri"], "file:///home/me/demo");
            assert_eq!(init["params"]["workspaceFolders"][0]["name"], "demo");
            assert_eq!(init["params"]["capabilities"]["general"]["positionEncodings"][0], "utf-8");
            rpc::write_message(&mut w, &json!({"jsonrpc": "2.0", "id": init["id"], "result": {"capabilities": {"positionEncoding": "utf-8", "textDocumentSync": {"openClose": true, "change": 2, "save": true}, "definitionProvider": true}}})).unwrap();
            assert_eq!(next()["method"], "initialized");
            // the document opened before the server was ready, with its latest text
            let open = next();
            assert_eq!(open["method"], "textDocument/didOpen");
            assert_eq!(open["params"]["textDocument"]["text"], "int b;");
            assert_eq!(open["params"]["textDocument"]["languageId"], "c");
            let change = next();
            assert_eq!(change["method"], "textDocument/didChange");
            assert_eq!(change["params"]["textDocument"]["version"], 3);
            assert_eq!(change["params"]["contentChanges"][0]["text"], "int c;");
            assert_eq!(next()["method"], "textDocument/didSave");
            let def = next();
            assert_eq!(def["method"], "textDocument/definition");
            rpc::write_message(&mut w, &json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": "file:///home/me/demo/a.c", "version": 3, "diagnostics": [{"range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 5}}, "severity": 2, "message": "unused"}]}})).unwrap();
            rpc::write_message(&mut w, &json!({"jsonrpc": "2.0", "method": "window/showMessage", "params": {"type": 1, "message": "no compile_commands.json"}})).unwrap();
            rpc::write_message(&mut w, &json!({"jsonrpc": "2.0", "id": def["id"], "result": []})).unwrap();
            assert_eq!(next()["method"], "textDocument/didClose");
            let shutdown = next();
            assert_eq!(shutdown["method"], "shutdown");
            rpc::write_message(&mut w, &json!({"jsonrpc": "2.0", "id": shutdown["id"], "result": null})).unwrap();
            assert_eq!(next()["method"], "exit");
        });
        let mut s = Session::start(Client::over(to_client_r, to_server_w, None), "/home/me/demo", Value::Null);
        let uri = "file:///home/me/demo/a.c";
        s.open(uri, "c", "int a;");
        s.change(uri, "int b;");
        assert!(s.request("textDocument/definition", json!({})).is_none(), "not ready yet");
        wait(&mut s, |o| matches!(o, Output::Ready));
        assert_eq!(s.caps.encoding, Encoding::Utf8);
        s.change(uri, "int c;");
        s.change(uri, "int c;");
        assert_eq!(s.version(uri), Some(3));
        s.save(uri);
        let id = s.request("textDocument/definition", json!({})).unwrap();
        // three outputs, in the order the server sent them (one poll may bring them all)
        let mut got = vec![];
        let start = Instant::now();
        while got.len() < 3 {
            got.extend(s.poll());
            assert!(start.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        match &got[0] {
            Output::Diagnostics { uri: u, version, list } => assert_eq!((u.as_str(), *version, list.len()), (uri, Some(3), 1)),
            o => panic!("{o:?}"),
        }
        match &got[1] {
            Output::Message { error, text } => assert!(*error && text.contains("compile_commands")),
            o => panic!("{o:?}"),
        }
        match &got[2] {
            Output::Response { id: got, result } => assert_eq!((*got, result.clone().unwrap()), (id, json!([]))),
            o => panic!("{o:?}"),
        }
        s.close(uri);
        assert!(!s.is_open(uri));
        s.stop(Duration::from_secs(5));
        server.join().unwrap();
        assert_eq!(s.state, State::Exited);
    }

    #[test]
    fn a_failed_start_is_told() {
        let (to_server_r, to_server_w) = io::pipe().unwrap();
        let (to_client_r, mut w) = io::pipe().unwrap();
        let server = std::thread::spawn(move || {
            let mut r = BufReader::new(to_server_r);
            let init = rpc::read_message(&mut r).unwrap().unwrap();
            rpc::write_message(&mut w, &json!({"jsonrpc": "2.0", "id": init["id"], "error": {"code": -32002, "message": "unsupported client"}})).unwrap();
        });
        let mut s = Session::start(Client::over(to_client_r, to_server_w, None), "E:/demo", Value::Null);
        match wait(&mut s, |o| matches!(o, Output::Failed(_))) {
            Output::Failed(m) => assert_eq!(m, "unsupported client"),
            _ => unreachable!(),
        }
        server.join().unwrap();
        assert!(!s.ready());
    }

    #[test]
    fn clangd_tells_its_encoding_its_own_way() {
        let (to_server_r, to_server_w) = io::pipe().unwrap();
        let (to_client_r, mut w) = io::pipe().unwrap();
        let server = std::thread::spawn(move || {
            let mut r = BufReader::new(to_server_r);
            let init = rpc::read_message(&mut r).unwrap().unwrap();
            assert_eq!(init["params"]["capabilities"]["offsetEncoding"][0], "utf-8");
            rpc::write_message(&mut w, &json!({"jsonrpc": "2.0", "id": init["id"], "result": {"capabilities": {"textDocumentSync": 2}, "offsetEncoding": "utf-8"}})).unwrap();
            rpc::read_message(&mut r).unwrap().unwrap()
        });
        let mut s = Session::start(Client::over(to_client_r, to_server_w, None), "E:/demo", Value::Null);
        wait(&mut s, |o| matches!(o, Output::Ready));
        assert_eq!(s.caps.encoding, Encoding::Utf8);
        assert_eq!(server.join().unwrap()["method"], "initialized");
    }
}

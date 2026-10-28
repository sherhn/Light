//! The server process and the threads around it: one writes what the IDE sends, one reads what the server says and
//! answers the questions the server asks of its client, one keeps the end of what it writes to stderr. Nothing here
//! ever blocks the UI: messages go out through a channel and come back as [`Event`]s.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::rpc;

/// Called after an event arrived, from the thread that read it (to wake the UI).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// How many lines of the server's stderr are kept for telling why it stopped.
const LOG_LINES: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug)]
pub enum Event {
    /// The answer to the request `id`.
    Response { id: u64, result: Result<Value, ResponseError> },
    Notification { method: String, params: Value },
    /// The server's output ended: it exited or crashed.
    Exited,
}

pub struct Client {
    out: Sender<Value>,
    events: Receiver<Event>,
    next_id: u64,
    child: Option<Child>,
    log: Arc<Mutex<VecDeque<String>>>,
}

/// The answer to a request the server sends its client: what a client that offers none of these features says.
fn answer(method: &str, params: &Value) -> Result<Value, (i64, &'static str)> {
    match method {
        // one setting per item asked for, and the IDE has none
        "workspace/configuration" => Ok(Value::Array(vec![Value::Null; params["items"].as_array().map_or(0, Vec::len)])),
        "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" | "window/showMessageRequest" | "workspace/workspaceFolders" => Ok(Value::Null),
        "workspace/applyEdit" => Ok(json!({"applied": false, "failureReason": "not supported"})),
        _ => Err((-32601, "method not found")),
    }
}

fn reader_loop(input: impl Read, out: Sender<Value>, events: Sender<Event>, wake: Option<Wake>) {
    let mut r = BufReader::new(input);
    let wake = || {
        if let Some(w) = &wake {
            w();
        }
    };
    while let Ok(Some(msg)) = rpc::read_message(&mut r) {
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").filter(|v| !v.is_null());
        let event = match (method, id) {
            // a request of the server's
            (Some(m), Some(id)) => {
                let reply = match answer(m, &msg["params"]) {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
                };
                let _ = out.send(reply);
                continue;
            }
            (Some(m), None) => Event::Notification { method: m.to_string(), params: msg.get("params").cloned().unwrap_or(Value::Null) },
            (None, Some(id)) => {
                // the IDE's ids are numbers; anything else is not an answer to it
                let Some(id) = id.as_u64() else { continue };
                let result = match msg.get("error") {
                    Some(e) if !e.is_null() => Err(ResponseError { code: e["code"].as_i64().unwrap_or(0), message: e["message"].as_str().unwrap_or("").to_string() }),
                    _ => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                Event::Response { id, result }
            }
            (None, None) => continue,
        };
        if events.send(event).is_err() {
            return;
        }
        wake();
    }
    let _ = events.send(Event::Exited);
    wake();
}

fn writer_loop(mut output: impl Write, messages: Receiver<Value>) {
    for m in messages {
        if rpc::write_message(&mut output, &m).is_err() {
            return;
        }
    }
}

impl Client {
    /// Talk to a server over these streams (its stdout and its stdin).
    pub fn over(input: impl Read + Send + 'static, output: impl Write + Send + 'static, wake: Option<Wake>) -> Client {
        let (out_tx, out_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let replies = out_tx.clone();
        std::thread::spawn(move || writer_loop(output, out_rx));
        std::thread::spawn(move || reader_loop(input, replies, ev_tx, wake));
        Client { out: out_tx, events: ev_rx, next_id: 1, child: None, log: Arc::default() }
    }

    /// Start the server `cmd` and talk to it over its stdin and stdout.
    pub fn spawn(mut cmd: Command, wake: Option<Wake>) -> io::Result<Client> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
            let _ = child.kill();
            return Err(io::Error::other("no pipes to the server"));
        };
        let mut c = Client::over(stdout, stdin, wake);
        // a full stderr pipe would stop the server: read it all, keep the end
        let log = c.log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line).trim_end().to_string();
                if line.is_empty() {
                    continue;
                }
                let mut l = log.lock().unwrap();
                if l.len() == LOG_LINES {
                    l.pop_front();
                }
                l.push_back(line);
            }
        });
        c.child = Some(child);
        Ok(c)
    }

    /// Send a request; its answer comes as [`Event::Response`] with the id returned.
    pub fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let _ = self.out.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        let _ = self.out.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// The process id of the server (of what was started: a shell or `wsl.exe` that runs it).
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Has the process ended?
    pub fn exited(&mut self) -> bool {
        self.child.as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None)))
    }

    /// The exit code of the process, waiting at most `timeout` for it to end (its output may close a moment before).
    /// The shell says 127 when the program is not installed, `cmd` 9009.
    pub fn exit_code(&mut self, timeout: std::time::Duration) -> Option<i32> {
        let c = self.child.as_mut()?;
        let start = std::time::Instant::now();
        loop {
            match c.try_wait() {
                Ok(Some(status)) => return status.code(),
                Ok(None) if start.elapsed() < timeout => std::thread::sleep(std::time::Duration::from_millis(10)),
                _ => return None,
            }
        }
    }

    /// The last lines the server wrote to stderr.
    pub fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().iter().cloned().collect()
    }

    /// End the process now.
    pub fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_event(c: &Client) -> Event {
        let start = Instant::now();
        loop {
            if let Some(e) = c.try_event() {
                return e;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "no event");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn requests_answers_and_the_servers_own_questions() {
        let (to_server_r, to_server_w) = io::pipe().unwrap();
        let (to_client_r, mut to_client_w) = io::pipe().unwrap();
        let woken = Arc::new(Mutex::new(0));
        let w = woken.clone();
        let mut c = Client::over(to_client_r, to_server_w, Some(Arc::new(move || *w.lock().unwrap() += 1)));
        let server = std::thread::spawn(move || {
            let mut r = BufReader::new(to_server_r);
            let req = rpc::read_message(&mut r).unwrap().unwrap();
            assert_eq!(req["method"], "textDocument/hover");
            // the server asks its own question first; the client answers it by itself
            rpc::write_message(&mut to_client_w, &json!({"jsonrpc": "2.0", "id": "cfg", "method": "workspace/configuration", "params": {"items": [{}, {}]}})).unwrap();
            let reply = rpc::read_message(&mut r).unwrap().unwrap();
            assert_eq!(reply["id"], "cfg");
            assert_eq!(reply["result"], json!([null, null]));
            rpc::write_message(&mut to_client_w, &json!({"jsonrpc": "2.0", "id": "x", "method": "custom/unknown"})).unwrap();
            assert_eq!(rpc::read_message(&mut r).unwrap().unwrap()["error"]["code"], -32601);
            rpc::write_message(&mut to_client_w, &json!({"jsonrpc": "2.0", "method": "window/logMessage", "params": {"message": "hi"}})).unwrap();
            rpc::write_message(&mut to_client_w, &json!({"jsonrpc": "2.0", "id": req["id"], "result": {"contents": "int x"}})).unwrap();
            rpc::write_message(&mut to_client_w, &json!({"jsonrpc": "2.0", "id": 99, "error": {"code": -32803, "message": "failed"}})).unwrap();
        });
        let id = c.request("textDocument/hover", json!({}));
        match wait_event(&c) {
            Event::Notification { method, params } => {
                assert_eq!(method, "window/logMessage");
                assert_eq!(params["message"], "hi");
            }
            e => panic!("{e:?}"),
        }
        match wait_event(&c) {
            Event::Response { id: got, result } => {
                assert_eq!(got, id);
                assert_eq!(result.unwrap()["contents"], "int x");
            }
            e => panic!("{e:?}"),
        }
        match wait_event(&c) {
            Event::Response { id: 99, result: Err(e) } => assert_eq!(e.message, "failed"),
            e => panic!("{e:?}"),
        }
        server.join().unwrap();
        // the server's end closed: the client says so
        assert!(matches!(wait_event(&c), Event::Exited));
        assert!(*woken.lock().unwrap() >= 4);
    }
}

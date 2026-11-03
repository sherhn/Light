//! The Debug Adapter Protocol: the messages a debugger of a module (an adapter such as vscode-js-debug) and the IDE
//! exchange - requests, their responses and events - framed with a `Content-Length` header, over a TCP connection.
//! Reading and writing happen on threads of their own; the IDE takes the messages that came in every frame.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// Read one message. The header ends with an empty line and must say how long the body is.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Value> {
    let mut size = None;
    let mut header = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        header += line.len();
        if header > 8192 {
            return Err(io::Error::other("the header of a message is too long"));
        }
        if line.trim().is_empty() {
            if size.is_some() {
                break;
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case("Content-Length")
        {
            size = value.trim().parse::<usize>().ok();
        }
    }
    let size = size.filter(|s| *s <= MAX_MESSAGE).ok_or_else(|| io::Error::other("a message without a valid Content-Length"))?;
    let mut body = vec![0; size];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}

pub fn write_message(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// What came from the adapter.
#[derive(Clone, Debug)]
pub enum Incoming {
    Message(Value),
    /// The connection ended (the reason).
    Closed(String),
}

/// A connection to an adapter: requests go out with their own sequence numbers, everything that comes in waits in
/// [`Connection::rx`].
pub struct Connection {
    tx: Sender<Value>,
    pub rx: Receiver<Incoming>,
    seq: i64,
    stream: Option<TcpStream>,
    /// An adapter that speaks on its standard input and output: it ends with the connection.
    child: Option<std::process::Child>,
}

impl Connection {
    /// Connect to an adapter listening at `addr`, trying until `timeout` (it may still be starting).
    pub fn connect(addr: SocketAddr, timeout: Duration) -> io::Result<Connection> {
        let deadline = Instant::now() + timeout;
        let stream = loop {
            match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                Ok(s) => break s,
                Err(e) if Instant::now() >= deadline => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        };
        stream.set_nodelay(true).ok();
        Connection::over(stream)
    }

    fn over(stream: TcpStream) -> io::Result<Connection> {
        let mut writer = stream.try_clone()?;
        let reader = stream.try_clone()?;
        let (tx, outgoing) = mpsc::channel::<Value>();
        let (incoming, rx) = mpsc::channel();
        let errors = incoming.clone();
        std::thread::spawn(move || {
            for value in outgoing {
                if let Err(e) = write_message(&mut writer, &value) {
                    let _ = errors.send(Incoming::Closed(e.to_string()));
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                match read_message(&mut reader) {
                    Ok(v) => {
                        if incoming.send(Incoming::Message(v)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = incoming.send(Incoming::Closed(if e.kind() == io::ErrorKind::UnexpectedEof { "the adapter closed the connection".into() } else { e.to_string() }));
                        break;
                    }
                }
            }
        });
        Ok(Connection { tx, rx, seq: 0, stream: Some(stream), child: None })
    }

    /// Start an adapter that speaks on its standard input and output (`gdb -i dap`). What it writes to its errors
    /// comes as output events of category `adapter`.
    pub fn spawn(mut command: std::process::Command) -> io::Result<Connection> {
        use std::process::Stdio;
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let mut writer = child.stdin.take().ok_or_else(|| io::Error::other("no input"))?;
        let reader = child.stdout.take().ok_or_else(|| io::Error::other("no output"))?;
        let errors_out = child.stderr.take().ok_or_else(|| io::Error::other("no errors"))?;
        let (tx, outgoing) = mpsc::channel::<Value>();
        let (incoming, rx) = mpsc::channel();
        let errors = incoming.clone();
        let log = incoming.clone();
        std::thread::spawn(move || {
            for value in outgoing {
                if let Err(e) = write_message(&mut writer, &value) {
                    let _ = errors.send(Incoming::Closed(e.to_string()));
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                match read_message(&mut reader) {
                    Ok(v) => {
                        if incoming.send(Incoming::Message(v)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = incoming.send(Incoming::Closed(if e.kind() == io::ErrorKind::UnexpectedEof { "the adapter ended".into() } else { e.to_string() }));
                        break;
                    }
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(errors_out);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                let text = line.replace('\0', "");
                if !text.trim().is_empty() {
                    let _ = log.send(Incoming::Message(json!({"type": "event", "event": "output", "body": {"category": "adapter", "output": text}})));
                }
                line.clear();
            }
        });
        Ok(Connection { tx, rx, seq: 0, stream: None, child: Some(child) })
    }

    /// Send a request; its response will carry the number returned.
    pub fn request(&mut self, command: &str, arguments: Value) -> i64 {
        self.seq += 1;
        let _ = self.tx.send(json!({"seq": self.seq, "type": "request", "command": command, "arguments": arguments}));
        self.seq
    }

    /// Answer a request of the adapter (a reverse request such as `startDebugging`).
    pub fn respond(&mut self, request: &Value, success: bool, body: Value) {
        self.seq += 1;
        let _ = self.tx.send(json!({"seq": self.seq, "type": "response", "request_seq": request["seq"], "command": request["command"], "success": success, "body": body}));
    }

    /// The messages that came in since the last call.
    pub fn take(&self) -> Vec<Incoming> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(s) = &self.stream {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// A TCP port nobody listens on now (for an adapter to listen on).
pub fn free_port() -> io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// `value` with the placeholders of `vars` (`{file}` -> ...) filled in its strings, at any depth.
pub fn fill(value: &Value, vars: &[(&str, &str)]) -> Value {
    match value {
        Value::String(s) => Value::String(vars.iter().fold(s.clone(), |acc, (k, v)| acc.replace(&format!("{{{k}}}"), v))),
        Value::Array(a) => Value::Array(a.iter().map(|v| fill(v, vars)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), fill(v, vars))).collect()),
        other => other.clone(),
    }
}

/// Is this message the response to request `seq`?
pub fn is_response(msg: &Value, seq: i64) -> bool {
    msg["type"] == "response" && msg["request_seq"].as_i64() == Some(seq)
}

/// Do two paths an adapter and the IDE give name the same file? On Windows the case of the letters (the drive's in
/// particular) and the kind of slashes do not matter.
pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| {
        let p = p.trim_start_matches(r"\\?\").replace('\\', "/");
        if cfg!(windows) { p.to_lowercase() } else { p }
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_framed_by_their_length_in_bytes() {
        let value = json!({"text": "Привет 🦀\r\n", "n": 1});
        let mut data = vec![];
        write_message(&mut data, &value).unwrap();
        write_message(&mut data, &Value::Null).unwrap();
        let mut reader = io::Cursor::new(data);
        assert_eq!(read_message(&mut reader).unwrap(), value);
        assert_eq!(read_message(&mut reader).unwrap(), Value::Null);
        assert!(read_message(&mut reader).is_err());
        for bad in ["Content-Length: 99999999999\r\n\r\n", "Other: 4\r\n\r\nnull", "Content-Length: 8\r\n\r\nnull"] {
            assert!(read_message(&mut io::Cursor::new(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_connection_sends_requests_and_answers_reverse_requests() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let adapter = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let init = read_message(&mut reader).unwrap();
            write_message(&mut writer, &json!({"seq": 1, "type": "response", "request_seq": init["seq"], "command": "initialize", "success": true, "body": {}})).unwrap();
            write_message(&mut writer, &json!({"seq": 2, "type": "request", "command": "startDebugging", "arguments": {"request": "launch"}})).unwrap();
            read_message(&mut reader).unwrap()
        });
        let mut c = Connection::connect(addr, Duration::from_secs(5)).unwrap();
        let seq = c.request("initialize", json!({"adapterID": "x"}));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = vec![];
        while got.len() < 2 && Instant::now() < deadline {
            got.extend(c.take().into_iter().filter_map(|i| if let Incoming::Message(m) = i { Some(m) } else { None }));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(is_response(&got[0], seq));
        assert_eq!(got[1]["command"], "startDebugging");
        c.respond(&got[1], true, json!({}));
        let answer = adapter.join().unwrap();
        assert_eq!((answer["type"].as_str(), answer["request_seq"].as_i64(), answer["success"].as_bool()), (Some("response"), Some(2), Some(true)));
    }

    #[test]
    fn placeholders_are_filled_at_any_depth_and_paths_compare() {
        let launch = json!({"program": "{file}", "args": ["--dir", "{dir}"], "env": {"X": "{dir}/x"}, "n": 1});
        assert_eq!(fill(&launch, &[("file", "/p/a.js"), ("dir", "/p")]), json!({"program": "/p/a.js", "args": ["--dir", "/p"], "env": {"X": "/p/x"}, "n": 1}));
        assert!(same_path("/a/b.js", "/a/b.js"));
        if cfg!(windows) {
            assert!(same_path(r"e:\light\prog.js", "E:/light/prog.js"));
        }
        assert!(free_port().unwrap() > 0);
    }
}

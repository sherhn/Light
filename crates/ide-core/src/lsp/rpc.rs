//! The wire format: JSON-RPC 2.0 messages, each after a `Content-Length` header and a blank line.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// The largest message read: a server that sends more is not a language server, or is broken.
const MAX_MESSAGE: usize = 64 << 20;

/// Write one message.
pub fn write_message(w: &mut impl Write, msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Read the next message; `None` when the stream ends between messages. Headers other than `Content-Length` are
/// skipped, and so is a line that is no header at all (`wsl.exe` may print a notice of its own before the server
/// says anything).
pub fn read_message(r: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        if r.read_until(b'\n', &mut line)? == 0 {
            return match len {
                None => Ok(None),
                Some(_) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the stream ended inside a header")),
            };
        }
        let text = String::from_utf8_lossy(&line);
        let l = text.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some((k, v)) = l.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = Some(v.trim().parse().map_err(|_| invalid(format!("bad Content-Length: {v:?}")))?);
        }
    }
    let n = len.unwrap_or_default();
    if n > MAX_MESSAGE {
        return Err(invalid(format!("a message of {n} bytes")));
    }
    let mut body = vec![0; n];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(|e| invalid(format!("not JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_go_there_and_back() {
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"})).unwrap();
        write_message(&mut buf, &json!({"jsonrpc": "2.0", "method": "exit", "params": {"name": "ünïcode"}})).unwrap();
        let mut r = io::BufReader::new(&buf[..]);
        assert_eq!(read_message(&mut r).unwrap().unwrap()["method"], "initialize");
        assert_eq!(read_message(&mut r).unwrap().unwrap()["params"]["name"], "ünïcode");
        assert!(read_message(&mut r).unwrap().is_none());
    }

    #[test]
    fn other_headers_and_stray_lines_are_skipped() {
        let body = r#"{"id":7,"result":null}"#;
        let text = format!("wsl: a notice about the proxy\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: {}\r\n\r\n{body}", body.len());
        let mut r = io::BufReader::new(text.as_bytes());
        assert_eq!(read_message(&mut r).unwrap().unwrap()["id"], 7);
    }

    #[test]
    fn a_broken_stream_is_an_error() {
        let mut r = io::BufReader::new("Content-Length: 50\r\n\r\n{\"id\":1}".as_bytes());
        assert!(read_message(&mut r).is_err());
        let mut r = io::BufReader::new("Content-Length: x\r\n\r\n".as_bytes());
        assert!(read_message(&mut r).is_err());
        let mut r = io::BufReader::new("Content-Length: 3\r\n\r\n{]}".as_bytes());
        assert!(read_message(&mut r).is_err());
    }
}

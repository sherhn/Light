//! HTTP requests written in `.http` files (the format of the HTTP clients of JetBrains IDEs and of VS Code's REST
//! Client): what the file asks for, its variables and environments, the `curl` that sends a request and the response
//! read back from what `curl` printed.
//!
//! ```text
//! @host = https://example.org
//! ### List the users
//! GET {{host}}/users?page=1
//! Accept: application/json
//!
//! ### @name create
//! POST {{host}}/users
//! Content-Type: application/json
//!
//! {"name": "{{$uuid}}"}
//! ```

use std::collections::BTreeMap;
use std::path::Path;

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"];

/// One request of a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// 0-based line of the request line (`GET https://...`).
    pub line: usize,
    /// The name after `###` or of a `# @name` comment (empty: none).
    pub name: String,
    pub method: String,
    /// The URL with the `?`/`&` continuation lines joined, variables not yet filled in.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// `< path`: the body is the content of this file (relative to the `.http` file).
    pub body_file: Option<String>,
    /// `# @no-redirect`: redirects are not followed.
    pub no_redirect: bool,
}

impl Request {
    /// What to call the request: its name, or `GET /path`.
    pub fn title(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        // the path of the URL; a URL that starts with a variable (`{{host}}/users`) has the path after it
        let after_host = match self.url.split_once("://") {
            Some((_, rest)) => rest,
            None if self.url.starts_with("{{") => self.url.split_once("}}").map_or("", |(_, rest)| rest),
            None => self.url.as_str(),
        };
        let path = after_host.find('/').map_or("/", |i| &after_host[i..]);
        format!("{} {}", self.method, path.split('?').next().unwrap_or(path))
    }
}

/// The requests of a file and the variables it defines (`@name = value`, in the order written).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpFile {
    pub vars: Vec<(String, String)>,
    pub requests: Vec<Request>,
}

fn request_line(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let (first, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    let (method, target) = if METHODS.contains(&first) { (first.to_string(), rest.trim()) } else { ("GET".to_string(), t) };
    // `HTTP/1.1` at the end of the line is the protocol, not part of the URL
    let target = target.rsplit_once(" HTTP/").map_or(target, |(u, _)| u).trim();
    let looks_like_url = target.contains("://") || target.starts_with('/') || target.starts_with("{{") || target.starts_with("localhost") || target.contains('.');
    (looks_like_url && !target.is_empty() && !target.contains(char::is_whitespace)).then(|| (method, target.to_string()))
}

/// Read a `.http` file.
pub fn parse(text: &str) -> HttpFile {
    let mut file = HttpFile::default();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let mut name = String::new();
    let mut no_redirect = false;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("###") {
            name = rest.trim().to_string();
            no_redirect = false;
            i += 1;
            continue;
        }
        if t.is_empty() {
            i += 1;
            continue;
        }
        if t.starts_with('#') || t.starts_with("//") {
            let tag = t.trim_start_matches(['#', '/']).trim();
            if let Some(n) = tag.strip_prefix("@name") {
                name = n.trim_start_matches([' ', '=']).trim().to_string();
            } else if tag == "@no-redirect" {
                no_redirect = true;
            }
            i += 1;
            continue;
        }
        if let Some(var) = t.strip_prefix('@')
            && let Some((k, v)) = var.split_once('=')
        {
            file.vars.push((k.trim().to_string(), v.trim().to_string()));
            i += 1;
            continue;
        }
        let Some((method, mut url)) = request_line(line) else {
            i += 1;
            continue;
        };
        let start = i;
        i += 1;
        // the query may go on over indented lines that start with `?` or `&`
        while i < lines.len() && lines[i].starts_with(char::is_whitespace) && matches!(lines[i].trim_start().chars().next(), Some('?' | '&')) {
            url.push_str(lines[i].trim());
            i += 1;
        }
        let mut headers = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() && !lines[i].trim().starts_with("###") {
            let h = lines[i].trim();
            if !(h.starts_with('#') || h.starts_with("//"))
                && let Some((k, v)) = h.split_once(':')
            {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
            i += 1;
        }
        let mut body = Vec::new();
        while i < lines.len() && !lines[i].trim().starts_with("###") {
            body.push(lines[i]);
            i += 1;
        }
        // a response handler (`> {% ... %}`, `> file.js`) or a redirect of the response (`>> file`) ends the body
        if let Some(cut) = body.iter().position(|l| l.starts_with("> ") || l.starts_with(">>") || l.trim() == ">") {
            body.truncate(cut);
        }
        while body.last().is_some_and(|l| l.trim().is_empty()) {
            body.pop();
        }
        while body.first().is_some_and(|l| l.trim().is_empty()) {
            body.remove(0);
        }
        let body_file = (body.len() == 1).then(|| body[0].trim().strip_prefix("< ").map(|p| p.trim().to_string())).flatten();
        let body = if body_file.is_some() { String::new() } else { body.join("\n") };
        file.requests.push(Request { line: start, name: std::mem::take(&mut name), method, url, headers, body, body_file, no_redirect });
        no_redirect = false;
    }
    file
}

/// The environments of the `.http` files of a folder: `http-client.env.json` (`{"dev": {"host": "..."}}`) with the
/// values of `http-client.private.env.json` over it.
pub fn environments(dir: &Path) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut envs: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for name in ["http-client.env.json", "http-client.private.env.json"] {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else { continue };
        let Ok(serde_json::Value::Object(all)) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        for (env, vars) in all {
            let Some(vars) = vars.as_object() else { continue };
            let target = envs.entry(env).or_default();
            for (k, v) in vars {
                target.insert(k.clone(), v.as_str().map(String::from).unwrap_or_else(|| v.to_string()));
            }
        }
    }
    envs
}

fn dynamic(name: &str) -> Option<String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let seed = now.as_nanos() as u64 ^ (std::process::id() as u64).rotate_left(32);
    let random = |n: u64| {
        // xorshift: not for secrets, only for test data
        let mut x = seed.wrapping_add(n.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    Some(match name {
        "$uuid" | "$random.uuid" => {
            let (a, b) = (random(1), random(2));
            format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", a >> 32, (a >> 16) & 0xffff, a & 0xfff, (b >> 48 & 0x3fff) | 0x8000, b & 0xffff_ffff_ffff)
        }
        "$timestamp" => now.as_secs().to_string(),
        "$randomInt" | "$random.integer" => (random(3) % 1000).to_string(),
        "$isoTimestamp" => {
            let secs = now.as_secs() as i64;
            let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
            // days since 1970-01-01 to a civil date (Howard Hinnant's algorithm)
            let z = days + 719_468;
            let era = z.div_euclid(146_097);
            let doe = z - era * 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let day = doy - (153 * mp + 2) / 5 + 1;
            let month = if mp < 10 { mp + 3 } else { mp - 9 };
            let year = yoe + era * 400 + i64::from(month <= 2);
            format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest / 60 % 60, rest % 60)
        }
        _ => return None,
    })
}

/// `text` with every `{{name}}` filled in: from the environment `env`, the file's variables (which may use each other
/// and the environment), or the dynamic ones (`$uuid`, `$timestamp`, `$isoTimestamp`, `$randomInt`).
pub fn substitute(text: &str, file_vars: &[(String, String)], env: &BTreeMap<String, String>) -> Result<String, String> {
    fn fill(text: &str, file_vars: &[(String, String)], env: &BTreeMap<String, String>, depth: usize) -> Result<String, String> {
        if depth > 8 {
            return Err("the variables refer to each other in a circle".into());
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find("{{") {
            out.push_str(&rest[..open]);
            let after = &rest[open + 2..];
            let Some(close) = after.find("}}") else {
                out.push_str(&rest[open..]);
                return Ok(out);
            };
            let name = after[..close].trim();
            let value = if let Some(v) = env.get(name) {
                fill(v, file_vars, env, depth + 1)?
            } else if let Some((_, v)) = file_vars.iter().rev().find(|(k, _)| k == name) {
                fill(v, file_vars, env, depth + 1)?
            } else if let Some(v) = dynamic(name) {
                v
            } else {
                return Err(format!("the variable {name:?} is not defined"));
            };
            out.push_str(&value);
            rest = &after[close + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }
    fill(text, file_vars, env, 0)
}

/// The arguments of the `curl` that sends `request` (variables already filled in). The body, when there is one, is
/// read from `body_path`; what `curl` prints is the response's head and body, then a line [`MARK`] with the status, the
/// time and the size.
pub fn curl_args(request: &Request, body_path: Option<&Path>) -> Vec<String> {
    let mut args: Vec<String> = ["-sS", "-i", "--max-time", "600", "--write-out"].into_iter().map(String::from).collect();
    args.push(format!("\n{MARK} %{{http_code}} %{{time_total}} %{{size_download}}\n"));
    if !request.no_redirect {
        args.push("-L".into());
    }
    if request.method == "HEAD" {
        args.push("--head".into());
    } else {
        args.extend(["-X".to_string(), request.method.clone()]);
    }
    for (k, v) in &request.headers {
        args.extend(["-H".to_string(), format!("{k}: {v}")]);
    }
    if let Some(p) = body_path {
        args.extend(["--data-binary".to_string(), format!("@{}", p.display())]);
    }
    args.extend(["--url".to_string(), request.url.clone()]);
    args
}

/// The line `curl` ends what it prints with.
pub const MARK: &str = "@@light-http@@";

/// A response read back from what `curl` printed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Response {
    /// `HTTP/1.1 200 OK` of the last response (after redirects).
    pub status_line: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub seconds: f64,
    pub size: u64,
}

impl Response {
    /// The body as a person reads it: JSON indented, anything else as it came.
    pub fn pretty_body(&self) -> String {
        let json = self.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().contains("json"));
        if (json || self.body.trim_start().starts_with(['{', '[']))
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&self.body)
        {
            return serde_json::to_string_pretty(&v).unwrap_or_else(|_| self.body.clone());
        }
        self.body.clone()
    }
}

/// Read the response out of the output of [`curl_args`]'s `curl` (`None` when it did not get as far as a response).
pub fn read_response(output: &str) -> Option<Response> {
    let output = output.replace("\r\n", "\n");
    let mark = output.rfind(MARK)?;
    let mut tail = output[mark + MARK.len()..].split_whitespace();
    let status: u16 = tail.next()?.parse().ok()?;
    let seconds = tail.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let size = tail.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    if status == 0 {
        return None;
    }
    let mut rest = output[..mark].strip_suffix('\n').unwrap_or(&output[..mark]);
    // the heads of redirects and of `100 Continue` come first: the last head is the response's
    loop {
        let Some(end) = rest.find("\n\n") else {
            // a head and no body (HEAD, 204)
            return Some(head_only(rest, status, seconds, size));
        };
        let next = &rest[end + 2..];
        if next.starts_with("HTTP/") {
            rest = next;
            continue;
        }
        let mut r = head_only(&rest[..end], status, seconds, size);
        r.body = next.to_string();
        return Some(r);
    }
}

fn head_only(head: &str, status: u16, seconds: f64, size: u64) -> Response {
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default().trim().to_string();
    let headers = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect();
    Response { status_line, status, headers, body: String::new(), seconds, size }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "@host = https://api.example.org\n@token = secret-{{env_part}}\n\n### List users\nGET {{host}}/users\n    ?page=1\n    &size=10\nAccept: application/json\n# a comment between the headers\nAuthorization: Bearer {{token}}\n\n### \n# @name create\nPOST {{host}}/users HTTP/1.1\nContent-Type: application/json\n\n{\n  \"id\": \"{{$uuid}}\"\n}\n\n> {% client.global.set(\"id\", response.body.id); %}\n\n###\n# @no-redirect\nhttps://example.org/plain\n\n###\nPUT /upload\n\n< ./data.json\n";

    #[test]
    fn requests_headers_bodies_and_names_are_read() {
        let f = parse(FILE);
        assert_eq!(f.vars, [("host".to_string(), "https://api.example.org".to_string()), ("token".to_string(), "secret-{{env_part}}".to_string())]);
        assert_eq!(f.requests.len(), 4);
        let list = &f.requests[0];
        assert_eq!((list.line, list.name.as_str(), list.method.as_str(), list.url.as_str()), (4, "List users", "GET", "{{host}}/users?page=1&size=10"));
        assert_eq!(list.headers, [("Accept".to_string(), "application/json".to_string()), ("Authorization".to_string(), "Bearer {{token}}".to_string())]);
        assert!(list.body.is_empty());
        let create = &f.requests[1];
        assert_eq!((create.name.as_str(), create.method.as_str(), create.url.as_str()), ("create", "POST", "{{host}}/users"));
        assert_eq!(create.body, "{\n  \"id\": \"{{$uuid}}\"\n}", "the response handler is not part of the body");
        let plain = &f.requests[2];
        assert!(plain.no_redirect && plain.method == "GET" && plain.title() == "GET /plain");
        assert_eq!(f.requests[3].body_file.as_deref(), Some("./data.json"));
        assert_eq!(f.requests[0].title(), "List users");
        assert_eq!(Request { method: "GET".into(), url: "{{host}}/a/b?x=1".into(), ..Default::default() }.title(), "GET /a/b");
        // prose is not a request
        assert!(parse("Some notes about the API\n").requests.is_empty());
    }

    #[test]
    fn variables_come_from_the_environment_the_file_and_dynamic_values() {
        let f = parse(FILE);
        let env: BTreeMap<String, String> = [("env_part".to_string(), "42".to_string())].into();
        assert_eq!(substitute("{{host}}/a?t={{token}}", &f.vars, &env).unwrap(), "https://api.example.org/a?t=secret-42");
        let env2: BTreeMap<String, String> = [("host".to_string(), "http://localhost:8080".to_string()), ("env_part".to_string(), "x".to_string())].into();
        assert_eq!(substitute("{{ host }}/a", &f.vars, &env2).unwrap(), "http://localhost:8080/a", "the environment wins");
        assert!(substitute("{{nope}}", &f.vars, &env).unwrap_err().contains("\"nope\""));
        let uuid = substitute("{{$uuid}}", &[], &BTreeMap::new()).unwrap();
        assert!(uuid.len() == 36 && uuid.chars().nth(14) == Some('4'), "{uuid}");
        let iso = substitute("{{$isoTimestamp}}", &[], &BTreeMap::new()).unwrap();
        assert!(iso.len() == 20 && iso.ends_with('Z') && iso.starts_with("20"), "{iso}");
        assert!(substitute("{{a}}", &[("a".into(), "{{b}}".into()), ("b".into(), "{{a}}".into())], &BTreeMap::new()).unwrap_err().contains("circle"));
        assert_eq!(substitute("no {{ end", &[], &BTreeMap::new()).unwrap(), "no {{ end");
    }

    #[test]
    fn environments_merge_the_private_file_over_the_shared_one() {
        let dir = crate::testutil::TempDir::new("http-env");
        std::fs::write(dir.path().join("http-client.env.json"), r#"{"dev":{"host":"http://localhost","port":8080},"prod":{"host":"https://x"}}"#).unwrap();
        std::fs::write(dir.path().join("http-client.private.env.json"), r#"{"dev":{"token":"t0p"}}"#).unwrap();
        let envs = environments(dir.path());
        assert_eq!(envs.keys().collect::<Vec<_>>(), ["dev", "prod"]);
        assert_eq!(envs["dev"]["port"], "8080");
        assert_eq!(envs["dev"]["token"], "t0p");
        assert!(environments(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn curl_gets_the_request_and_its_output_becomes_the_response() {
        let r = Request { method: "POST".into(), url: "http://h/x".into(), headers: vec![("A".into(), "b: c".into())], ..Default::default() };
        let args = curl_args(&r, Some(Path::new("/tmp/body")));
        assert!(args.windows(2).any(|w| w == ["-X", "POST"]) && args.windows(2).any(|w| w == ["-H", "A: b: c"]) && args.windows(2).any(|w| w == ["--data-binary", "@/tmp/body"]));
        assert_eq!(args.last().map(String::as_str), Some("http://h/x"));
        assert!(args.contains(&"-L".to_string()) && !curl_args(&Request { no_redirect: true, ..r.clone() }, None).contains(&"-L".to_string()));
        assert!(curl_args(&Request { method: "HEAD".into(), ..r }, None).contains(&"--head".to_string()));

        let out = format!("HTTP/1.1 301 Moved\r\nLocation: /y\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{{\"a\":[1,2]}}\n{MARK} 200 0.25 9\n");
        let resp = read_response(&out).unwrap();
        assert_eq!((resp.status, resp.status_line.as_str(), resp.size), (200, "HTTP/1.1 200 OK", 9));
        assert_eq!(resp.headers, [("Content-Type".to_string(), "application/json".to_string())]);
        assert_eq!(resp.pretty_body(), "{\n  \"a\": [\n    1,\n    2\n  ]\n}");
        let head = read_response(&format!("HTTP/2 204\nServer: x\n\n{MARK} 204 0.01 0\n")).unwrap();
        assert!(head.body.is_empty() && head.status == 204);
        assert!(read_response(&format!("curl: (6) Could not resolve host\n{MARK} 000 0 0\n")).is_none());
        let text = read_response(&format!("HTTP/1.1 404 Not Found\n\nnot here\n{MARK} 404 0.1 9\n")).unwrap();
        assert_eq!((text.status, text.pretty_body().as_str()), (404, "not here"));
    }
}

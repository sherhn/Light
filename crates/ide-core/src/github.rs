//! GitHub for "Get from Version Control": the signed-in account and its repositories, through the REST API.
//!
//! No HTTP library is linked: requests go through `curl` (on Windows 10+, macOS and nearly every Linux), and the
//! token travels in curl's config on stdin, never on a command line where other programs could read it. Git gets the
//! token for a clone through environment variables of that one process. Nothing is written to disk by default;
//! [`remember_token`] hands the login to Git's own credential helper, and only when the user asks for it.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const DEFAULT_API: &str = "https://api.github.com";
/// Where a user creates a token that can list and clone private repositories.
pub const TOKEN_URL: &str = "https://github.com/settings/tokens/new?scopes=repo&description=Light";
/// Repositories are listed in pages of 100, at most this many pages (1000 repositories).
pub const MAX_PAGES: usize = 10;

const REQUEST_TIMEOUT_SECS: u64 = 30;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repo {
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub description: String,
    pub private: bool,
    pub fork: bool,
    pub archived: bool,
    pub language: Option<String>,
    pub stars: u64,
    /// ISO 8601 time of the last push (`2026-10-21T12:34:56Z`), empty when unknown.
    pub pushed_at: String,
    pub clone_url: String,
    pub default_branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub login: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// `curl` is not installed.
    NoCurl,
    /// The request did not get an answer (no internet, DNS, timeout).
    Network(String),
    /// 401: the token is wrong, expired or revoked.
    Unauthorized,
    /// 404: no such user (or no access).
    NotFound,
    /// 403 / 429 with GitHub's rate limit message.
    RateLimited,
    Http(u16, String),
    /// The answer is not what the API documents.
    Parse(String),
}

/// What to list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoSource {
    /// The signed-in account's repositories, including private, collaborator and organization ones.
    Mine,
    /// The public repositories of any user.
    User(String),
}

/// A GitHub login is letters, digits and hyphens (older accounts may have `_` / `.`); anything else would only
/// be a typo or an attempt to change the request path.
pub fn valid_login(s: &str) -> bool {
    !s.is_empty() && s.len() <= 39 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

// ---------------------------------------------------------------------------------------------- parsing

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The repositories of one page of `/user/repos` or `/users/NAME/repos`.
pub fn parse_repos(body: &str) -> Result<Vec<Repo>, ApiError> {
    let v: Value = serde_json::from_str(body).map_err(|e| ApiError::Parse(e.to_string()))?;
    let list = v.as_array().ok_or_else(|| ApiError::Parse("a list of repositories was expected".into()))?;
    Ok(list
        .iter()
        .filter_map(|r| {
            let full_name = text(r, "full_name");
            let clone_url = text(r, "clone_url");
            if full_name.is_empty() || clone_url.is_empty() {
                return None;
            }
            let name = r.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| full_name.rsplit('/').next().unwrap_or("").to_string());
            let owner = r.get("owner").and_then(|o| o.get("login")).and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| full_name.split('/').next().unwrap_or("").to_string());
            Some(Repo {
                full_name,
                name,
                owner,
                description: text(r, "description"),
                private: flag(r, "private"),
                fork: flag(r, "fork"),
                archived: flag(r, "archived"),
                language: r.get("language").and_then(Value::as_str).filter(|l| !l.is_empty()).map(str::to_string),
                stars: r.get("stargazers_count").and_then(Value::as_u64).unwrap_or(0),
                pushed_at: text(r, "pushed_at"),
                clone_url,
                default_branch: text(r, "default_branch"),
            })
        })
        .collect())
}

pub fn parse_account(body: &str) -> Result<Account, ApiError> {
    let v: Value = serde_json::from_str(body).map_err(|e| ApiError::Parse(e.to_string()))?;
    let login = text(&v, "login");
    if login.is_empty() {
        return Err(ApiError::Parse("the answer has no login".into()));
    }
    Ok(Account { login, name: v.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).map(str::to_string) })
}

/// The error for an HTTP answer other than 200.
pub fn classify(status: u16, body: &str) -> ApiError {
    let message = serde_json::from_str::<Value>(body).ok().and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
    match status {
        401 => ApiError::Unauthorized,
        404 => ApiError::NotFound,
        403 | 429 if message.to_ascii_lowercase().contains("rate limit") => ApiError::RateLimited,
        _ => ApiError::Http(status, message),
    }
}

// ---------------------------------------------------------------------------------------------- time

/// Seconds since 1970-01-01 of an ISO 8601 UTC time (`2026-10-21T12:34:56Z`).
pub fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Since {
    JustNow,
    Minutes(u64),
    Hours(u64),
    Days(u64),
    Months(u64),
    Years(u64),
}

/// How long before `now` (epoch seconds) the ISO time `iso` was.
pub fn since(iso: &str, now: i64) -> Option<Since> {
    let secs = (now - parse_iso(iso)?).max(0) as u64;
    Some(match secs {
        0..=59 => Since::JustNow,
        60..=3599 => Since::Minutes(secs / 60),
        3600..=86_399 => Since::Hours(secs / 3600),
        86_400..=2_591_999 => Since::Days(secs / 86_400),
        2_592_000..=31_535_999 => Since::Months(secs / 2_592_000),
        _ => Since::Years(secs / 31_536_000),
    })
}

// ---------------------------------------------------------------------------------------------- filtering

/// Indices of the repositories that match `query` (case-insensitive, every word in the name, description or
/// language), and the optional exclusions.
pub fn filter(repos: &[Repo], query: &str, include_forks: bool, include_archived: bool) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    repos
        .iter()
        .enumerate()
        .filter(|(_, r)| (include_forks || !r.fork) && (include_archived || !r.archived))
        .filter(|(_, r)| {
            let hay = format!("{} {} {}", r.full_name, r.description, r.language.as_deref().unwrap_or("")).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

// ---------------------------------------------------------------------------------------------- base64

/// Standard base64 (for the Basic credentials handed to git).
pub fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Environment for one `git clone` of `url` that authenticates to GitHub with `token`. Empty for any other host: the
/// token is never sent anywhere but github.com. The values live only in that process's environment.
pub fn git_auth_env(url: &str, token: &str) -> Vec<(String, String)> {
    if !url.starts_with("https://github.com/") || token.is_empty() {
        return vec![];
    }
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "http.https://github.com/.extraheader".into()),
        ("GIT_CONFIG_VALUE_0".into(), format!("Authorization: Basic {}", base64(format!("x-access-token:{token}").as_bytes()))),
    ]
}

// ---------------------------------------------------------------------------------------------- HTTP through curl

fn config_quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' | '\r' => {}
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// curl's config (read from stdin) for a GET of `url`; the token is only ever in here.
pub fn curl_config(url: &str, token: Option<&str>) -> String {
    let mut c = String::new();
    c.push_str(&format!("url = {}\n", config_quote(url)));
    c.push_str("header = \"Accept: application/vnd.github+json\"\n");
    c.push_str("header = \"X-GitHub-Api-Version: 2022-11-28\"\n");
    c.push_str("header = \"User-Agent: Light\"\n");
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        c.push_str(&format!("header = {}\n", config_quote(&format!("Authorization: Bearer {t}"))));
    }
    c.push_str("write-out = \"\\n%{http_code}\"\n");
    c
}

fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd;
}

/// Run `cmd` with `stdin_text`, killing it after `timeout`. Returns (stdout, stderr, exit code).
fn run(mut cmd: Command, stdin_text: &str, timeout: Duration) -> std::io::Result<(String, String, Option<i32>)> {
    no_window(&mut cmd);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_text.as_bytes());
    }
    let mut readers = Vec::new();
    for mut s in [Box::new(child.stdout.take().expect("piped")) as Box<dyn Read + Send>, Box::new(child.stderr.take().expect("piped"))] {
        readers.push(std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = s.read_to_end(&mut b);
            String::from_utf8_lossy(&b).into_owned()
        }));
    }
    let deadline = Instant::now() + timeout;
    let code = loop {
        match child.try_wait()? {
            Some(st) => break st.code(),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let mut it = readers.into_iter().map(|r| r.join().unwrap_or_default());
    Ok((it.next().unwrap_or_default(), it.next().unwrap_or_default(), code))
}

/// The GitHub REST API at `base`.
pub struct Api {
    base: String,
}

impl Api {
    pub fn new(base: &str) -> Api {
        Api { base: base.trim_end_matches('/').to_string() }
    }

    /// `LIGHT_GITHUB_API` (a test server) or api.github.com.
    pub fn from_env() -> Api {
        Api::new(&std::env::var("LIGHT_GITHUB_API").or_else(|_| std::env::var("AIR_GITHUB_API")).unwrap_or_else(|_| DEFAULT_API.to_string()))
    }

    /// GET `path` (starting with `/`); returns the HTTP status and body.
    pub fn get(&self, path: &str, token: Option<&str>) -> Result<(u16, String), ApiError> {
        let mut cmd = Command::new("curl");
        cmd.args(["-sS", "--max-time", &REQUEST_TIMEOUT_SECS.to_string(), "--connect-timeout", "10", "--config", "-"]);
        let url = format!("{}{path}", self.base);
        let (out, err, code) = match run(cmd, &curl_config(&url, token), Duration::from_secs(REQUEST_TIMEOUT_SECS + 10)) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ApiError::NoCurl),
            Err(e) => return Err(ApiError::Network(e.to_string())),
        };
        if code != Some(0) {
            let msg = err.trim();
            return Err(ApiError::Network(if msg.is_empty() { "no answer".into() } else { msg.to_string() }));
        }
        let (body, status) = out.rsplit_once('\n').ok_or_else(|| ApiError::Parse("empty answer".into()))?;
        let status: u16 = status.trim().parse().map_err(|_| ApiError::Parse("no HTTP status".into()))?;
        Ok((status, body.to_string()))
    }

    fn get_ok(&self, path: &str, token: Option<&str>) -> Result<String, ApiError> {
        match self.get(path, token)? {
            (200, body) => Ok(body),
            (status, body) => Err(classify(status, &body)),
        }
    }

    /// The account `token` belongs to.
    pub fn account(&self, token: &str) -> Result<Account, ApiError> {
        parse_account(&self.get_ok("/user", Some(token))?)
    }

    /// Any user's public profile (also checks that the login exists).
    pub fn user(&self, login: &str, token: Option<&str>) -> Result<Account, ApiError> {
        if !valid_login(login) {
            return Err(ApiError::NotFound);
        }
        parse_account(&self.get_ok(&format!("/users/{login}"), token)?)
    }

    /// List repositories page by page, newest push first; `on_page` gets each page as soon as it arrived. Returns
    /// how many repositories there were.
    pub fn repos(&self, source: &RepoSource, token: Option<&str>, on_page: &mut dyn FnMut(&[Repo])) -> Result<usize, ApiError> {
        let mut total = 0;
        for page in 1..=MAX_PAGES {
            let path = match source {
                RepoSource::Mine => format!("/user/repos?per_page=100&sort=pushed&direction=desc&affiliation=owner,collaborator,organization_member&page={page}"),
                RepoSource::User(u) if valid_login(u) => format!("/users/{u}/repos?per_page=100&sort=pushed&direction=desc&page={page}"),
                RepoSource::User(_) => return Err(ApiError::NotFound),
            };
            let items = parse_repos(&self.get_ok(&path, token)?)?;
            total += items.len();
            if !items.is_empty() {
                on_page(&items);
            }
            if items.len() < 100 {
                break;
            }
        }
        Ok(total)
    }
}

// ---------------------------------------------------------------------------------------------- saved logins

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenSource {
    /// Typed (or pasted) by the user in this session.
    Entered,
    /// `gh auth token` of the GitHub CLI.
    GhCli,
    /// The login Git itself has saved for github.com (Git Credential Manager, macOS keychain, ...).
    GitCredential,
    /// `LIGHT_GITHUB_FAKE_TOKEN`, for the UI tests.
    Test,
}

/// `username` and `password` of `git credential fill` output.
pub fn parse_credential_output(text: &str) -> Option<(String, String)> {
    let get = |key: &str| text.lines().find_map(|l| l.trim_end_matches('\r').strip_prefix(key)).map(str::to_string);
    let password = get("password=").filter(|p| !p.is_empty() && !p.contains(char::is_whitespace))?;
    Some((get("username=").unwrap_or_default(), password))
}

/// A token that is already on this computer: the GitHub CLI's, else the one Git has saved for github.com. Asks
/// nothing and never opens a window; a stale token is caught by the check against `/user` that follows.
pub fn find_saved_token() -> Option<(TokenSource, String)> {
    if let Ok(t) = std::env::var("LIGHT_GITHUB_FAKE_TOKEN").or_else(|_| std::env::var("AIR_GITHUB_FAKE_TOKEN")) {
        return (!t.is_empty()).then_some((TokenSource::Test, t));
    }
    if std::env::var("LIGHT_GITHUB_AUTODETECT").or_else(|_| std::env::var("AIR_GITHUB_AUTODETECT")).is_ok_and(|v| v == "0") {
        return None;
    }
    let mut gh = Command::new("gh");
    gh.args(["auth", "token"]);
    if let Ok((out, _, Some(0))) = run(gh, "", Duration::from_secs(10)) {
        let t = out.trim();
        if !t.is_empty() && !t.contains(char::is_whitespace) {
            return Some((TokenSource::GhCli, t.to_string()));
        }
    }
    let mut git = Command::new("git");
    git.args(["credential", "fill"]).env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never").env("GIT_ASKPASS", "").env("LC_ALL", "C");
    if let Ok((out, _, Some(0))) = run(git, "protocol=https\nhost=github.com\n\n", Duration::from_secs(15))
        && let Some((_, token)) = parse_credential_output(&out)
    {
        return Some((TokenSource::GitCredential, token));
    }
    None
}

// ---------------------------------------------------------------------------------------------- browser sign-in

/// How long GitHub's authorization page may stay unanswered before the wait is given up.
const BROWSER_LOGIN_TIMEOUT: Duration = Duration::from_secs(600);

/// `git credential-manager ARGS` (Git Credential Manager ships with Git for Windows and is installed on macOS and
/// Linux by many setups). `LIGHT_FAKE_GCM`, a Python script, replaces it in the UI tests.
fn gcm_command(args: &[&str]) -> Command {
    let mut c = match std::env::var_os("LIGHT_FAKE_GCM").or_else(|| std::env::var_os("AIR_FAKE_GCM")) {
        Some(script) => {
            let mut c = Command::new(std::env::var_os("LIGHT_FAKE_GCM_PYTHON").or_else(|| std::env::var_os("AIR_FAKE_GCM_PYTHON")).unwrap_or_else(|| "python".into()));
            c.arg(script);
            c
        }
        None => {
            let mut c = Command::new("git");
            c.arg("credential-manager");
            c
        }
    };
    c.args(args);
    c
}

/// Account names (one per line) of `git-credential-manager github list`.
pub fn parse_accounts(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.contains(char::is_whitespace)).map(str::to_string).collect()
}

/// The account a browser sign-in added: the one that was not there before, else the first one.
pub fn pick_account(before: &[String], after: &[String]) -> Option<String> {
    after.iter().find(|a| !before.contains(a)).or(after.first()).cloned()
}

/// The GitHub accounts Git Credential Manager knows; `None` when it is not installed.
pub fn gcm_accounts() -> Option<Vec<String>> {
    match run(gcm_command(&["github", "list"]), "", Duration::from_secs(20)) {
        Ok((out, _, Some(0))) => Some(parse_accounts(&out)),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginError {
    /// Git Credential Manager is not available.
    NoGcm,
    Cancelled,
    TimedOut,
    Failed(String),
}

/// Sign in to GitHub in the browser, like PyCharm: Git Credential Manager opens GitHub's authorization page, the user
/// signs in there (two-factor included) and the account lands in Git's credential store. Blocks until the page was
/// answered; set `cancel` to give up. Returns the account that signed in.
pub fn browser_login(cancel: &AtomicBool) -> Result<String, LoginError> {
    let before = gcm_accounts().ok_or(LoginError::NoGcm)?;
    let mut cmd = gcm_command(&["github", "login", "--browser"]);
    no_window(&mut cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { LoginError::NoGcm } else { LoginError::Failed(e.to_string()) })?;
    let mut readers = Vec::new();
    for mut s in [Box::new(child.stdout.take().expect("piped")) as Box<dyn Read + Send>, Box::new(child.stderr.take().expect("piped"))] {
        readers.push(std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = s.read_to_end(&mut b);
            String::from_utf8_lossy(&b).into_owned()
        }));
    }
    let deadline = Instant::now() + BROWSER_LOGIN_TIMEOUT;
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LoginError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(LoginError::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(LoginError::Failed(e.to_string())),
        }
    };
    let mut it = readers.into_iter().map(|r| r.join().unwrap_or_default());
    let (out, err) = (it.next().unwrap_or_default(), it.next().unwrap_or_default());
    if !status.success() {
        let last = format!("{err}\n{out}").lines().map(str::trim).rev().find(|l| !l.is_empty()).unwrap_or("the sign-in did not finish").to_string();
        return Err(LoginError::Failed(last));
    }
    let after = gcm_accounts().unwrap_or_default();
    pick_account(&before, &after).ok_or_else(|| LoginError::Failed("no account was added".into()))
}

/// The token Git has saved for `username` at github.com (no prompts, no windows).
pub fn credential_for(username: &str) -> Option<String> {
    if std::env::var_os("LIGHT_FAKE_GCM").or_else(|| std::env::var_os("AIR_FAKE_GCM")).is_some() {
        return std::env::var("LIGHT_FAKE_GCM_TOKEN").or_else(|_| std::env::var("AIR_FAKE_GCM_TOKEN")).ok().filter(|t| !t.is_empty());
    }
    if username.contains(['\n', '\r']) {
        return None;
    }
    let mut git = Command::new("git");
    git.args(["credential", "fill"]).env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never").env("GIT_ASKPASS", "").env("LC_ALL", "C");
    let input = format!("protocol=https\nhost=github.com\nusername={username}\n\n");
    match run(git, &input, Duration::from_secs(15)) {
        Ok((out, _, Some(0))) => parse_credential_output(&out).map(|(_, t)| t),
        _ => None,
    }
}

/// Save the login in Git's credential helper (`git credential approve`), so Git and Light find it next time.
/// Only called when the user ticks "remember".
pub fn remember_token(login: &str, token: &str) -> Result<(), String> {
    if login.contains(['\n', '\r']) || token.contains(['\n', '\r']) {
        return Err("invalid characters in the login or token".into());
    }
    let mut git = Command::new("git");
    git.args(["credential", "approve"]).env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never");
    let input = format!("protocol=https\nhost=github.com\nusername={login}\npassword={token}\n\n");
    match run(git, &input, Duration::from_secs(20)) {
        Ok((_, _, Some(0))) => Ok(()),
        Ok((_, err, _)) => Err(err.trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    const SAMPLE: &str = r#"[
      {"full_name":"octocat/Hello-World","name":"Hello-World","owner":{"login":"octocat"},"description":"My first repository","private":false,"fork":false,"archived":false,"language":"Lisp","stargazers_count":80,"pushed_at":"2026-10-20T10:00:00Z","clone_url":"https://github.com/octocat/Hello-World.git","default_branch":"main"},
      {"full_name":"octocat/secret","name":"secret","owner":{"login":"octocat"},"description":null,"private":true,"fork":true,"archived":true,"language":null,"stargazers_count":0,"pushed_at":null,"clone_url":"https://github.com/octocat/secret.git","default_branch":"trunk"},
      {"full_name":"broken","name":"x"}
    ]"#;

    #[test]
    fn parses_repositories_and_skips_unusable_entries() {
        let r = parse_repos(SAMPLE).unwrap();
        assert_eq!(r.len(), 2, "an entry without a clone URL is dropped");
        assert_eq!(r[0], Repo { full_name: "octocat/Hello-World".into(), name: "Hello-World".into(), owner: "octocat".into(), description: "My first repository".into(), private: false, fork: false, archived: false, language: Some("Lisp".into()), stars: 80, pushed_at: "2026-10-20T10:00:00Z".into(), clone_url: "https://github.com/octocat/Hello-World.git".into(), default_branch: "main".into() });
        assert!(r[1].private && r[1].fork && r[1].archived);
        assert_eq!((r[1].description.as_str(), r[1].language.clone(), r[1].pushed_at.as_str()), ("", None, ""));
        assert!(matches!(parse_repos("{\"message\":\"x\"}"), Err(ApiError::Parse(_))));
        assert!(matches!(parse_repos("not json"), Err(ApiError::Parse(_))));
        assert_eq!(parse_repos("[]").unwrap(), vec![]);
    }

    #[test]
    fn parses_accounts_and_classifies_errors() {
        assert_eq!(parse_account(r#"{"login":"octocat","name":"The Octocat"}"#).unwrap(), Account { login: "octocat".into(), name: Some("The Octocat".into()) });
        assert_eq!(parse_account(r#"{"login":"octocat","name":null}"#).unwrap().name, None);
        assert!(parse_account("{}").is_err());
        assert_eq!(classify(401, r#"{"message":"Bad credentials"}"#), ApiError::Unauthorized);
        assert_eq!(classify(404, "{}"), ApiError::NotFound);
        assert_eq!(classify(403, r#"{"message":"API rate limit exceeded for 1.2.3.4."}"#), ApiError::RateLimited);
        assert_eq!(classify(403, r#"{"message":"Resource not accessible"}"#), ApiError::Http(403, "Resource not accessible".into()));
        assert_eq!(classify(500, "oops"), ApiError::Http(500, String::new()));
    }

    #[test]
    fn logins_are_checked_before_they_reach_a_path() {
        assert!(valid_login("octocat") && valid_login("a-b_c.d") && valid_login("A1"));
        assert!(!valid_login("") && !valid_login("a/b") && !valid_login("a b") && !valid_login("a?x=1") && !valid_login(&"x".repeat(40)));
    }

    #[test]
    fn iso_times_and_ages() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_iso("2026-10-21T12:34:56Z"), Some(1_792_586_096));
        assert_eq!(parse_iso("garbage"), None);
        assert_eq!(parse_iso("2026-13-01T00:00:00Z"), None);
        let now = parse_iso("2026-10-21T12:00:00Z").unwrap();
        assert_eq!(since("2026-10-21T11:59:30Z", now), Some(Since::JustNow));
        assert_eq!(since("2026-10-21T11:15:00Z", now), Some(Since::Minutes(45)));
        assert_eq!(since("2026-10-21T07:00:00Z", now), Some(Since::Hours(5)));
        assert_eq!(since("2026-10-18T12:00:00Z", now), Some(Since::Days(3)));
        assert_eq!(since("2026-07-21T12:00:00Z", now), Some(Since::Months(3)));
        assert_eq!(since("2024-10-21T12:00:00Z", now), Some(Since::Years(2)));
        assert_eq!(since("2027-01-01T00:00:00Z", now), Some(Since::JustNow), "a clock a little behind is not negative");
        assert_eq!(since("", now), None);
    }

    #[test]
    fn filtering_by_words_forks_and_archives() {
        let r = parse_repos(SAMPLE).unwrap();
        assert_eq!(filter(&r, "", true, true), vec![0, 1]);
        assert_eq!(filter(&r, "", false, true), vec![0], "forks hidden");
        assert_eq!(filter(&r, "", true, false), vec![0], "archived hidden");
        assert_eq!(filter(&r, "hello lisp", true, true), vec![0], "every word, in name or language");
        assert_eq!(filter(&r, "FIRST", true, true), vec![0], "case-insensitive, in the description");
        assert!(filter(&r, "nothing like this", true, true).is_empty());
    }

    #[test]
    fn base64_matches_the_standard_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"x-access-token:ghp_abc"), "eC1hY2Nlc3MtdG9rZW46Z2hwX2FiYw==");
    }

    #[test]
    fn the_token_goes_to_git_only_for_github() {
        let env = git_auth_env("https://github.com/o/r.git", "tok");
        let get = |k: &str| env.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        assert_eq!(get("GIT_CONFIG_COUNT").as_deref(), Some("1"));
        assert_eq!(get("GIT_CONFIG_KEY_0").as_deref(), Some("http.https://github.com/.extraheader"));
        assert_eq!(get("GIT_CONFIG_VALUE_0").unwrap(), format!("Authorization: Basic {}", base64(b"x-access-token:tok")));
        assert!(git_auth_env("https://gitlab.com/o/r.git", "tok").is_empty());
        assert!(git_auth_env("https://github.com.evil.example/o/r.git", "tok").is_empty());
        assert!(git_auth_env("git@github.com:o/r.git", "tok").is_empty());
        assert!(git_auth_env(r"C:\local\repo", "tok").is_empty());
        assert!(git_auth_env("https://github.com/o/r.git", "").is_empty());
    }

    #[test]
    fn the_token_is_only_in_the_curl_config_with_values_quoted() {
        let c = curl_config("https://api.github.com/user", Some("ghp_secret"));
        assert!(c.contains("url = \"https://api.github.com/user\""));
        assert!(c.contains("header = \"Authorization: Bearer ghp_secret\""));
        assert!(c.contains("write-out = \"\\n%{http_code}\""));
        assert!(!curl_config("https://x", None).contains("Authorization"));
        let nasty = curl_config("https://x", Some("a\"b\\c\nd"));
        assert!(nasty.contains(r#"Bearer a\"b\\cd""#), "{nasty}");
    }

    #[test]
    fn gcm_account_lists_are_parsed_and_the_new_account_is_found() {
        assert_eq!(parse_accounts("sherhn\noctocat\n"), vec!["sherhn", "octocat"]);
        assert_eq!(parse_accounts("\n  \nsome words here\n"), Vec::<String>::new());
        assert_eq!(parse_accounts("one\r\ntwo\r\n"), vec!["one", "two"]);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(pick_account(&s(&["a"]), &s(&["a", "b"])), Some("b".into()), "the new one");
        assert_eq!(pick_account(&s(&["a"]), &s(&["a"])), Some("a".into()), "signing in again keeps the account");
        assert_eq!(pick_account(&[], &s(&["x"])), Some("x".into()));
        assert_eq!(pick_account(&s(&["a"]), &[]), None);
    }

    #[test]
    fn credential_output_is_read_and_junk_rejected() {
        let out = "protocol=https\nhost=github.com\nusername=octocat\npassword=gho_abc123\n";
        assert_eq!(parse_credential_output(out), Some(("octocat".into(), "gho_abc123".into())));
        assert_eq!(parse_credential_output("username=a\r\npassword=tok\r\n"), Some(("a".into(), "tok".into())));
        assert_eq!(parse_credential_output("username=a\n"), None);
        assert_eq!(parse_credential_output("password=\n"), None);
        assert_eq!(parse_credential_output("password=two words\n"), None);
    }

    /// A one-thread HTTP server: answers each request with the next canned (status, body) and records the
    /// request line and headers.
    fn serve(answers: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (status, body) in answers {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut reader = BufReader::new(s.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                log.lock().unwrap().push(request);
                let reason = if status == 200 { "OK" } else { "Error" };
                let _ = write!(s, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        (base, seen)
    }

    fn curl_available() -> bool {
        Command::new("curl").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }

    #[test]
    fn talks_to_a_server_through_curl_with_the_token_in_a_header() {
        if !curl_available() {
            return;
        }
        let (base, seen) = serve(vec![(200, r#"{"login":"octocat","name":"Mona"}"#.into()), (401, r#"{"message":"Bad credentials"}"#.into())]);
        let api = Api::new(&base);
        assert_eq!(api.account("ghp_good").unwrap().login, "octocat");
        assert_eq!(api.account("ghp_bad"), Err(ApiError::Unauthorized));
        let requests = seen.lock().unwrap().clone();
        assert!(requests[0].starts_with("GET /user HTTP/1.1"), "{}", requests[0]);
        assert!(requests[0].to_lowercase().contains("authorization: bearer ghp_good"), "{}", requests[0]);
        assert!(requests[0].to_lowercase().contains("user-agent: light"));
    }

    #[test]
    fn lists_repositories_page_by_page_until_a_short_page() {
        if !curl_available() {
            return;
        }
        let repo = |i: usize| format!(r#"{{"full_name":"o/r{i}","name":"r{i}","owner":{{"login":"o"}},"clone_url":"https://github.com/o/r{i}.git","default_branch":"main"}}"#);
        let page = |from: usize, n: usize| format!("[{}]", (from..from + n).map(repo).collect::<Vec<_>>().join(","));
        let (base, seen) = serve(vec![(200, page(0, 100)), (200, page(100, 100)), (200, page(200, 7))]);
        let api = Api::new(&base);
        let mut pages = vec![];
        let total = api.repos(&RepoSource::Mine, Some("tok"), &mut |p| pages.push(p.len())).unwrap();
        assert_eq!((total, pages), (207, vec![100, 100, 7]));
        let requests = seen.lock().unwrap().clone();
        assert!(requests[0].contains("GET /user/repos?per_page=100&sort=pushed&direction=desc&affiliation=owner,collaborator,organization_member&page=1"), "{}", requests[0]);
        assert!(requests[2].contains("page=3"));
    }

    #[test]
    fn public_listing_uses_the_user_path_and_rejects_bad_logins_without_a_request() {
        if !curl_available() {
            return;
        }
        let (base, seen) = serve(vec![(200, "[]".into()), (404, r#"{"message":"Not Found"}"#.into())]);
        let api = Api::new(&base);
        assert_eq!(api.repos(&RepoSource::User("octocat".into()), None, &mut |_| {}), Ok(0));
        assert!(seen.lock().unwrap()[0].starts_with("GET /users/octocat/repos?per_page=100"));
        assert!(!seen.lock().unwrap()[0].to_lowercase().contains("authorization"), "no token, no header");
        assert_eq!(api.repos(&RepoSource::User("a/../b".into()), None, &mut |_| {}), Err(ApiError::NotFound));
        assert_eq!(api.user("nobody", None), Err(ApiError::NotFound));
        assert_eq!(seen.lock().unwrap().len(), 2, "the bad login never became a request");
    }

    #[test]
    fn an_unreachable_server_is_a_network_error() {
        if !curl_available() {
            return;
        }
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        assert!(matches!(Api::new(&base).account("t"), Err(ApiError::Network(_))));
    }
}

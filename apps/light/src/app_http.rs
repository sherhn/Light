//! The HTTP client: a send button next to every request of a `.http` file (the language of a module whose engine is
//! `http`), the request sent by `curl` in a Run tab of its own, and the response shown there as a person reads it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use ide_core::http::{self, Request};
use lisp_runtime::discover::Launch;

use crate::app::App;
use crate::app_run::RunRequest;
use crate::editor_view::{GutterIcon, Sev};
use crate::gutter::GutterAction;
use crate::i18n::{self, tr};
use crate::icons::Icon;
use crate::state::{LogKind, LogLine};

impl App {
    /// Is document `idx` a file of requests: its language is one of a module with the HTTP engine?
    fn is_http_doc(&self, idx: usize) -> bool {
        let Some(doc) = self.docs.get(idx).filter(|d| d.diff.is_none() && d.page.is_none()) else { return false };
        let Some(language) = doc.ed.language() else { return false };
        self.mods.installed.iter().any(|m| m.enabled && m.manifest.builtin == "http" && m.manifest.languages.iter().any(|l| l.id == language.id()))
    }

    fn http_requests(&self, idx: usize) -> Vec<Request> {
        if !self.is_http_doc(idx) {
            return vec![];
        }
        http::parse(self.docs[idx].ed.text()).requests
    }

    pub fn http_icons(&mut self, idx: usize) -> Vec<GutterIcon> {
        let pal = self.pal;
        self.http_requests(idx).into_iter().map(|r| GutterIcon { line: r.line, icon: Icon::Play, color: pal.ok, tip: i18n::fmt("Send '{}'", &[&r.title()]) }).collect()
    }

    pub fn http_actions(&mut self, idx: usize, line: usize) -> Vec<(Icon, String, GutterAction)> {
        let Some(request) = self.http_requests(idx).into_iter().find(|r| r.line == line) else { return vec![] };
        let Some(path) = self.docs[idx].path.clone() else {
            return vec![(Icon::Play, i18n::fmt("Send '{}'", &[&request.title()]), GutterAction::SendRequest(PathBuf::new(), line, None))];
        };
        let envs = path.parent().map(http::environments).unwrap_or_default();
        let mut items = vec![(Icon::Play, i18n::fmt("Send '{}'", &[&request.title()]), GutterAction::SendRequest(path.clone(), line, None))];
        for env in envs.keys() {
            items.push((Icon::Globe, i18n::fmt("Send with '{}'", &[env]), GutterAction::SendRequest(path.clone(), line, Some(env.clone()))));
        }
        items
    }

    /// Send the request on `line` of the file `path` (the active document when the path is empty), with the variables of
    /// environment `env`.
    pub fn send_request(&mut self, path: &Path, line: usize, env: Option<&str>) {
        let idx = if path.as_os_str().is_empty() { Some(self.active) } else { self.docs.iter().position(|d| d.path.as_deref() == Some(path)) };
        let text = match idx {
            Some(i) => self.docs[i].ed.text().to_string(),
            None => std::fs::read_to_string(path).unwrap_or_default(),
        };
        let file = http::parse(&text);
        let Some(request) = file.requests.iter().find(|r| r.line == line).cloned() else {
            self.toast(tr("There is no request on that line any more"), Sev::Weak);
            return;
        };
        let dir = path.parent().map(Path::to_path_buf).or_else(|| self.project_root().map(Path::to_path_buf)).unwrap_or_default();
        let envs = http::environments(&dir);
        let vars: BTreeMap<String, String> = match env {
            Some(e) => match envs.get(e) {
                Some(v) => v.clone(),
                None => {
                    self.toast(i18n::fmt("No environment '{}' in http-client.env.json", &[&e]), Sev::Error);
                    return;
                }
            },
            None => BTreeMap::new(),
        };
        let fill = |s: &str| http::substitute(s, &file.vars, &vars);
        let filled = (|| -> Result<Request, String> {
            let mut r = request.clone();
            r.url = fill(&r.url)?;
            r.headers = r.headers.iter().map(|(k, v)| Ok((fill(k)?, fill(v)?))).collect::<Result<_, String>>()?;
            r.body = match &r.body_file {
                Some(f) => std::fs::read_to_string(dir.join(fill(f)?)).map_err(|e| format!("{f}: {e}"))?,
                None => fill(&r.body)?,
            };
            if !r.url.contains("://") {
                r.url = format!("http://{}", r.url.trim_start_matches('/'));
            }
            Ok(r)
        })();
        let request = match filled {
            Ok(r) => r,
            Err(e) => {
                self.toast(i18n::fmt("Cannot send the request: {}", &[&e]), Sev::Error);
                return;
            }
        };
        let body_path = if request.body.is_empty() {
            None
        } else {
            static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!("light-http-{}-{}.body", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            if let Err(e) = std::fs::write(&p, &request.body) {
                self.toast(i18n::fmt("Cannot send the request: {}", &[&e]), Sev::Error);
                return;
            }
            Some(p)
        };
        let mut cmd = Command::new("curl");
        cmd.args(http::curl_args(&request, body_path.as_deref())).current_dir(&dir);
        let title = format!("HTTP {}", request.title());
        let key = format!("http:{}:{line}", path.display());
        let shown = format!("{} {}", request.method, request.url);
        if self.start_run(key, title.clone(), Launch { cmd, cleanup: None }, &title, dir, None) {
            self.run.log.push(LogLine { kind: LogKind::Input, text: shown });
            self.run_request = Some(RunRequest::Http(path.to_path_buf(), line, env.map(String::from), body_path));
        } else if let Some(p) = body_path {
            let _ = std::fs::remove_file(p);
        }
    }

    /// A request's `curl` ended: its output becomes the response (status, headers, body), or stays as `curl`'s error.
    pub fn http_finished(&mut self, code: Option<i32>) {
        let Some(RunRequest::Http(_, _, _, body)) = self.run_request.clone() else { return };
        if let Some(p) = body {
            let _ = std::fs::remove_file(p);
        }
        // the first lines are the run's own (where it ran, the request), the rest is what curl printed
        let keep = self.run.log.iter().take_while(|l| matches!(l.kind, LogKind::Info | LogKind::Input)).count();
        let output: String = self.run.log[keep..].iter().filter(|l| !matches!(l.kind, LogKind::Info)).map(|l| l.text.as_str()).collect();
        let Some(response) = http::read_response(&output) else {
            if code == Some(0) {
                self.run.exit_code = Some(1);
            }
            self.status = tr("The request got no response").into();
            return;
        };
        self.run.log.truncate(keep);
        let ok = response.status < 400;
        self.run.log.push(LogLine { kind: if ok { LogKind::Value } else { LogKind::Error }, text: response.status_line.clone() });
        for (k, v) in &response.headers {
            self.run.log.push(LogLine { kind: LogKind::Trace, text: format!("{k}: {v}") });
        }
        self.run.log.push(LogLine { kind: LogKind::Output, text: String::new() });
        for line in response.pretty_body().lines() {
            self.run.log.push(LogLine { kind: LogKind::Output, text: line.to_string() });
        }
        let summary = i18n::fmt("{0} in {1} ms, {2}", &[&response.status, &((response.seconds * 1000.0).round() as u64), &crate::file_info::size(response.size)]);
        self.run.log.push(LogLine { kind: if ok { LogKind::Info } else { LogKind::Error }, text: summary.clone() });
        // the tab shows a failure for a client or server error
        self.run.exit_code = Some(if ok { 0 } else { i32::from(response.status) });
        self.status = summary;
    }
}

//! The GitHub session behind "Get from Version Control > GitHub": who is signed in, their repositories, and the
//! background requests that fill them. The token lives in memory only; with "remember" it is also handed to Git's
//! own credential helper (never to a file of ours). Results arrive on a channel and [`App::poll_github`] applies them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use ide_core::github::{Account, Api, ApiError, LoginError, Repo, RepoSource, TokenSource};

use crate::app::App;
use crate::editor_view::Sev;
use crate::i18n::{self, tr};

/// What the repository list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// The repositories of the signed-in account.
    Mine,
    /// The public repositories of another user.
    User(String),
}

enum Msg {
    /// Result of looking for a login that is already on this computer, then verifying it.
    SignedIn { generation: u64, token: String, source: TokenSource, result: Result<Account, ApiError> },
    /// No saved login exists.
    NothingSaved(u64),
    /// The sign-in in the browser did not end with a login.
    LoginFailed(u64, LoginError),
    /// Whose repositories are being listed.
    Shown(u64, Account),
    Page(u64, Vec<Repo>),
    Done(u64, Result<usize, ApiError>),
    Remembered(Result<(), String>),
}

pub struct GithubState {
    /// The signed-in account.
    pub account: Option<Account>,
    pub token: Option<String>,
    pub source: Option<TokenSource>,
    /// Whose list is on screen (the signed-in account, or a user looked up by name).
    pub shown: Option<Account>,
    pub listing: Option<Listing>,
    pub repos: Vec<Repo>,
    pub loading: bool,
    pub signing_in: bool,
    pub error: Option<ApiError>,
    /// The user pressed "Sign in with the saved login" and none was found.
    pub nothing_saved: bool,
    /// Set while the browser sign-in waits for the user; setting it to true gives the wait up.
    pub browser_cancel: Option<Arc<AtomicBool>>,
    pub login_error: Option<LoginError>,
    /// The less common ways to sign in (token, public repositories) are unfolded.
    pub more: bool,
    // ---- inputs of the sign-in form and the list
    pub token_input: String,
    pub user_input: String,
    pub remember: bool,
    pub filter: String,
    pub include_forks: bool,
    pub include_archived: bool,
    /// Screen rectangles of the visible repository rows (full name, row), for the UI tests.
    pub rects: Vec<(String, eframe::egui::Rect)>,
    generation: u64,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl Default for GithubState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        GithubState {
            account: None,
            token: None,
            source: None,
            shown: None,
            listing: None,
            repos: vec![],
            loading: false,
            signing_in: false,
            error: None,
            nothing_saved: false,
            browser_cancel: None,
            login_error: None,
            more: false,
            token_input: String::new(),
            user_input: String::new(),
            remember: false,
            filter: String::new(),
            include_forks: true,
            include_archived: true,
            rects: vec![],
            generation: 0,
            tx,
            rx,
        }
    }
}

impl GithubState {
    pub fn signed_in(&self) -> bool {
        self.account.is_some() && self.token.is_some()
    }

    pub fn busy(&self) -> bool {
        self.loading || self.signing_in
    }

    /// Indices into `repos` of the rows that pass the filter.
    pub fn visible(&self) -> Vec<usize> {
        ide_core::github::filter(&self.repos, &self.filter, self.include_forks, self.include_archived)
    }

    /// A login that persists across launches: the CLI's or Git's own, not one typed for this session.
    fn persistent(source: TokenSource) -> bool {
        matches!(source, TokenSource::GhCli | TokenSource::GitCredential)
    }
}

/// The text for a failed browser sign-in; `None` when the user cancelled.
pub fn login_error_text(e: &LoginError) -> Option<String> {
    match e {
        LoginError::Cancelled => None,
        LoginError::NoGcm => Some(tr("Git Credential Manager was not found. It comes with Git for Windows; or sign in with a token below.").to_string()),
        LoginError::TimedOut => Some(tr("The sign-in in the browser was not finished in time. Try again.").to_string()),
        LoginError::Failed(m) => Some(i18n::fmt("The sign-in did not finish: {}", &[&crate::ui_chrome::elide(m, 160)])),
    }
}

/// The text for an API error, in the interface language.
pub fn error_text(e: &ApiError) -> String {
    match e {
        ApiError::NoCurl => tr("curl was not found. Install curl (it ships with Windows 10 and later) to list GitHub repositories.").to_string(),
        ApiError::Network(m) => i18n::fmt("Cannot reach GitHub: {}", &[&crate::ui_chrome::elide(m.lines().next().unwrap_or(""), 160)]),
        ApiError::Unauthorized => tr("GitHub rejected the login: the token is wrong, expired or was revoked.").to_string(),
        ApiError::NotFound => tr("No such GitHub user.").to_string(),
        ApiError::RateLimited => tr("GitHub's request limit is reached. Sign in with a token or try again later.").to_string(),
        ApiError::Http(s, m) => i18n::fmt("GitHub answered {0}: {1}", &[s, &crate::ui_chrome::elide(m, 160)]),
        ApiError::Parse(m) => i18n::fmt("Unexpected answer from GitHub: {}", &[&crate::ui_chrome::elide(m, 160)]),
    }
}

impl App {
    /// Sign in with a login that is already on this computer (GitHub CLI, or the one Git saved for github.com).
    pub fn github_sign_in_saved(&mut self) {
        if self.github.signing_in {
            return;
        }
        self.github.signing_in = true;
        self.github.nothing_saved = false;
        self.github.error = None;
        let (tx, generation) = (self.github.tx.clone(), self.github.generation);
        std::thread::spawn(move || {
            match ide_core::github::find_saved_token() {
                Some((source, token)) => {
                    let result = Api::from_env().account(&token);
                    let _ = tx.send(Msg::SignedIn { generation, token, source, result });
                }
                None => {
                    let _ = tx.send(Msg::NothingSaved(generation));
                }
            };
        });
    }

    /// Sign in the way PyCharm does: GitHub's authorization page opens in the browser (through Git Credential
    /// Manager), the user signs in there, and the account comes back here.
    pub fn github_sign_in_browser(&mut self) {
        if self.github.signing_in {
            return;
        }
        self.github.signing_in = true;
        self.github.nothing_saved = false;
        self.github.error = None;
        self.github.login_error = None;
        let cancel = Arc::new(AtomicBool::new(false));
        self.github.browser_cancel = Some(cancel.clone());
        let (tx, generation) = (self.github.tx.clone(), self.github.generation);
        std::thread::spawn(move || match ide_core::github::browser_login(&cancel) {
            Ok(login) => match ide_core::github::credential_for(&login) {
                Some(token) => {
                    let result = Api::from_env().account(&token);
                    let _ = tx.send(Msg::SignedIn { generation, token, source: TokenSource::GitCredential, result });
                }
                None => {
                    let _ = tx.send(Msg::LoginFailed(generation, LoginError::Failed("the saved login could not be read".into())));
                }
            },
            Err(e) => {
                let _ = tx.send(Msg::LoginFailed(generation, e));
            }
        });
    }

    /// Stop waiting for the browser.
    pub fn github_cancel_browser(&mut self) {
        if let Some(c) = &self.github.browser_cancel {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// Sign in with a personal access token typed by the user; `remember` also saves it in Git's credential helper.
    pub fn github_sign_in_token(&mut self, token: &str, remember: bool) {
        let token = token.trim().to_string();
        if token.is_empty() || self.github.signing_in {
            return;
        }
        self.github.signing_in = true;
        self.github.nothing_saved = false;
        self.github.error = None;
        let (tx, generation) = (self.github.tx.clone(), self.github.generation);
        std::thread::spawn(move || {
            let result = Api::from_env().account(&token);
            if remember && let Ok(a) = &result {
                let _ = tx.send(Msg::Remembered(ide_core::github::remember_token(&a.login, &token)));
            }
            let _ = tx.send(Msg::SignedIn { generation, token, source: TokenSource::Entered, result });
        });
    }

    /// Forget the session (the saved login of the GitHub CLI or Git is left alone).
    pub fn github_sign_out(&mut self) {
        let g = &mut self.github;
        g.generation += 1;
        g.account = None;
        g.token = None;
        g.source = None;
        g.shown = None;
        g.listing = None;
        g.repos.clear();
        g.loading = false;
        g.signing_in = false;
        g.error = None;
        g.login_error = None;
        g.token_input.clear();
        if let Some(c) = g.browser_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.settings.github_autologin = false;
        self.save_settings();
    }

    /// List the repositories of `listing` in the background; earlier requests are ignored from now on.
    pub fn github_load(&mut self, listing: Listing) {
        let token = self.github.token.clone();
        if listing == Listing::Mine && token.is_none() {
            return;
        }
        if let Listing::User(u) = &listing
            && !ide_core::github::valid_login(u)
        {
            self.github.error = Some(ApiError::NotFound);
            return;
        }
        let g = &mut self.github;
        g.generation += 1;
        g.loading = true;
        g.error = None;
        g.repos.clear();
        g.listing = Some(listing.clone());
        if listing == Listing::Mine {
            g.shown = g.account.clone();
        } else {
            g.shown = None;
        }
        let (tx, generation) = (g.tx.clone(), g.generation);
        std::thread::spawn(move || {
            let api = Api::from_env();
            let source = match &listing {
                Listing::Mine => RepoSource::Mine,
                Listing::User(u) => {
                    match api.user(u, token.as_deref()) {
                        Ok(a) => {
                            let _ = tx.send(Msg::Shown(generation, a));
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::Done(generation, Err(e)));
                            return;
                        }
                    }
                    RepoSource::User(u.clone())
                }
            };
            let result = api.repos(&source, token.as_deref(), &mut |page| {
                let _ = tx.send(Msg::Page(generation, page.to_vec()));
            });
            let _ = tx.send(Msg::Done(generation, result));
        });
    }

    pub fn github_refresh(&mut self) {
        if let Some(l) = self.github.listing.clone() {
            self.github_load(l);
        }
    }

    /// Show the public repositories of `login`.
    pub fn github_show_user(&mut self, login: &str) {
        let login = login.trim().trim_start_matches('@').to_string();
        self.settings.github_user = login.clone();
        self.github_load(Listing::User(login));
    }

    pub fn poll_github(&mut self) {
        let mut sign_in_done = false;
        while let Ok(m) = self.github.rx.try_recv() {
            match m {
                Msg::SignedIn { generation, token, source, result } => {
                    if generation != self.github.generation {
                        continue;
                    }
                    self.github.signing_in = false;
                    self.github.browser_cancel = None;
                    match result {
                        Ok(account) => {
                            self.settings.github_user = account.login.clone();
                            self.settings.github_autologin = GithubState::persistent(source);
                            self.github.token = Some(token);
                            self.github.source = Some(source);
                            self.github.account = Some(account);
                            self.github.token_input.clear();
                            sign_in_done = true;
                        }
                        Err(e) => self.github.error = Some(e),
                    }
                }
                Msg::NothingSaved(generation) => {
                    if generation == self.github.generation {
                        self.github.signing_in = false;
                        self.github.nothing_saved = true;
                    }
                }
                Msg::LoginFailed(generation, e) => {
                    if generation == self.github.generation {
                        self.github.signing_in = false;
                        self.github.browser_cancel = None;
                        if e != LoginError::Cancelled {
                            self.github.login_error = Some(e);
                        }
                    }
                }
                Msg::Shown(generation, a) => {
                    if generation == self.github.generation {
                        self.github.shown = Some(a);
                    }
                }
                Msg::Page(generation, page) => {
                    if generation == self.github.generation {
                        self.github.repos.extend(page);
                    }
                }
                Msg::Done(generation, result) => {
                    if generation == self.github.generation {
                        self.github.loading = false;
                        if let Err(e) = result {
                            self.github.error = Some(e);
                        }
                    }
                }
                Msg::Remembered(r) => match r {
                    Ok(()) => self.toast(tr("The GitHub login is saved in Git's credential store"), Sev::Weak),
                    Err(e) => self.toast(i18n::fmt("Could not save the login: {}", &[&crate::ui_chrome::elide(&e, 120)]), Sev::Error),
                },
            }
        }
        if sign_in_done {
            self.save_settings();
            self.github_load(Listing::Mine);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_logins_that_survive_a_restart_turn_autologin_on() {
        assert!(GithubState::persistent(TokenSource::GhCli));
        assert!(GithubState::persistent(TokenSource::GitCredential));
        assert!(!GithubState::persistent(TokenSource::Entered));
        assert!(!GithubState::persistent(TokenSource::Test));
    }

    #[test]
    fn visible_rows_follow_the_filter() {
        let mut g = GithubState::default();
        let repo = |n: &str, fork: bool| Repo { full_name: format!("me/{n}"), name: n.into(), owner: "me".into(), fork, clone_url: format!("https://github.com/me/{n}.git"), ..Repo::default() };
        g.repos = vec![repo("alpha", false), repo("beta", true), repo("alphabet", false)];
        assert_eq!(g.visible(), vec![0, 1, 2]);
        g.filter = "alpha".into();
        assert_eq!(g.visible(), vec![0, 2]);
        g.filter.clear();
        g.include_forks = false;
        assert_eq!(g.visible(), vec![0, 2]);
    }

    #[test]
    fn signing_out_clears_the_session_and_ignores_late_answers() {
        let mut app = App::new(ide_core::settings::Settings::default());
        app.github.token = Some("t".into());
        app.github.account = Some(Account { login: "me".into(), name: None });
        app.github.repos = vec![Repo::default()];
        let before = app.github.generation;
        app.github_sign_out();
        assert!(!app.github.signed_in() && app.github.repos.is_empty());
        assert_eq!(app.github.generation, before + 1);
        // an answer of the old generation arrives after the sign-out
        let tx = app.github.tx.clone();
        tx.send(Msg::Page(before, vec![Repo::default()])).unwrap();
        tx.send(Msg::SignedIn { generation: before, token: "t".into(), source: TokenSource::Entered, result: Ok(Account { login: "me".into(), name: None }) }).unwrap();
        app.poll_github();
        assert!(app.github.repos.is_empty() && !app.github.signed_in(), "stale answers are dropped");
    }

    #[test]
    fn cancelling_the_browser_sign_in_shows_no_error_but_other_failures_do() {
        assert_eq!(login_error_text(&LoginError::Cancelled), None);
        assert!(login_error_text(&LoginError::NoGcm).unwrap().contains("Credential Manager"));
        assert!(login_error_text(&LoginError::Failed("boom".into())).unwrap().contains("boom"));
        let mut app = App::new(ide_core::settings::Settings::default());
        let generation = app.github.generation;
        let tx = app.github.tx.clone();
        app.github.signing_in = true;
        tx.send(Msg::LoginFailed(generation, LoginError::Cancelled)).unwrap();
        app.poll_github();
        assert!(!app.github.signing_in && app.github.login_error.is_none());
        app.github.signing_in = true;
        tx.send(Msg::LoginFailed(generation, LoginError::TimedOut)).unwrap();
        app.poll_github();
        assert_eq!(app.github.login_error, Some(LoginError::TimedOut));
    }

    #[test]
    fn errors_are_described_for_the_user() {
        assert!(error_text(&ApiError::Unauthorized).contains("token"));
        assert!(error_text(&ApiError::NoCurl).contains("curl"));
        assert!(error_text(&ApiError::Http(500, "boom".into())).contains("500"));
    }
}

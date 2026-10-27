//! The GitHub tab of "Get from Version Control": sign in (a login already on this computer, or a token), then the
//! repositories of the account - or the public ones of any user - as a searchable list; one click picks a
//! repository to clone, a double click clones it.

use crate::scrollbars::FadingScroll as _;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align2, Color32, CornerRadius, Margin, Rect, RichText, Sense, Vec2, pos2, vec2};
use ide_core::github::{self, Repo, Since};

use crate::app::App;
use crate::app_github::{Listing, error_text};
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::state::CloneState;
use crate::theme::Palette;
use crate::widgets::{self, TextField};

const ROW_H: f32 = 54.0;

fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// "3 d ago" in the interface language.
fn age_text(iso: &str) -> String {
    match github::since(iso, now_epoch()) {
        None => String::new(),
        Some(Since::JustNow) => tr("just now").to_string(),
        Some(Since::Minutes(n)) => i18n::fmt("{} min ago", &[&n]),
        Some(Since::Hours(n)) => i18n::fmt("{} h ago", &[&n]),
        Some(Since::Days(n)) => i18n::fmt("{} d ago", &[&n]),
        Some(Since::Months(n)) => i18n::fmt("{} mo ago", &[&n]),
        Some(Since::Years(n)) => i18n::fmt("{} y ago", &[&n]),
    }
}

fn badge(ui: &egui::Ui, pal: &Palette, x: f32, y: f32, text: &str, color: Color32) -> f32 {
    let g = ui.painter().layout_no_wrap(text.to_string(), fonts::ui(10.5), color);
    let r = Rect::from_min_size(pos2(x, y - 8.0), vec2(g.size().x + 12.0, 16.0));
    ui.painter().rect_filled(r, CornerRadius::same(8), color.gamma_multiply(0.18));
    ui.painter().galley(pos2(r.min.x + 6.0, r.center().y - g.size().y / 2.0), g, color);
    let _ = pal;
    r.width() + 6.0
}

/// The round "avatar": the first letter of the login.
fn avatar(ui: &mut egui::Ui, pal: &Palette, login: &str, size: f32) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().circle_filled(r.center(), size / 2.0, pal.accent.gamma_multiply(0.28));
    let initial = login.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    ui.painter().text(r.center(), Align2::CENTER_CENTER, initial, fonts::semibold(size * 0.46), pal.accent);
}

impl App {
    /// Draw the GitHub tab. Returns true when the user asked to clone right away (double click).
    pub fn draw_github_tab(&mut self, ui: &mut egui::Ui, st: &mut CloneState, running: bool) -> bool {
        if self.github.user_input.is_empty() {
            self.github.user_input = self.settings.github_user.clone();
        }
        if self.github.busy() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        let showing = self.github.listing.is_some() && (self.github.shown.is_some() || self.github.loading || !self.github.repos.is_empty() || self.github.error.is_some());
        if !showing {
            self.draw_github_sign_in(ui, running);
            return false;
        }
        self.draw_github_list(ui, st, running)
    }

    // ------------------------------------------------------------------ sign in

    fn draw_github_sign_in(&mut self, ui: &mut egui::Ui, running: bool) {
        let pal = self.pal;
        let mut browser_login = false;
        let mut cancel_login = false;
        let mut saved = false;
        let mut token_in: Option<(String, bool)> = None;
        let mut user_in: Option<String> = None;
        let mut token_page = false;
        let waiting = self.github.browser_cancel.is_some();
        let busy = self.github.signing_in || running;
        // an error of the token or user form unfolds the more options so it is seen next to its field
        let more = self.github.more || self.github.error.is_some();
        egui::Frame::new().fill(pal.hover).corner_radius(CornerRadius::same(14)).inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // ---- in the browser, like PyCharm
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    // the text wraps before the button instead of running under it
                    ui.set_max_width((ui.available_width() - 220.0).max(200.0));
                    ui.label(RichText::new(tr("Sign in with GitHub")).font(fonts::semibold(14.0)));
                    let hint = if waiting { tr("Finish signing in on the GitHub page that opened in your browser, then come back here.") } else { tr("GitHub's sign-in page opens in your browser (two-factor login works there). Light never sees your password.") };
                    ui.label(RichText::new(hint).color(pal.text_dim).size(11.5));
                });
                widgets::right_row(ui, 38.0, |ui| {
                    if waiting {
                        if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                            cancel_login = true;
                        }
                        ui.add(egui::Spinner::new().size(18.0).color(pal.accent));
                    } else {
                        ui.add_enabled_ui(!busy, |ui| {
                            if widgets::primary_button(ui, &pal, tr("Sign in in the browser")).clicked() {
                                browser_login = true;
                            }
                        });
                    }
                });
            });
            if let Some(m) = self.github.login_error.as_ref().and_then(crate::app_github::login_error_text) {
                ui.add_space(6.0);
                ui.label(RichText::new(m).color(pal.error).size(12.0));
            }
            ui.add_space(10.0);
            // ---- a login that is already on this computer
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Already signed in to GitHub on this computer?")).color(pal.text_dim).size(12.0));
                widgets::right_row(ui, 30.0, |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if widgets::secondary_button(ui, &pal, tr("Use the saved login")).on_hover_text(tr("Uses the GitHub CLI or the login Git has saved for github.com. Nothing is asked or stored.")).clicked() {
                            saved = true;
                        }
                    });
                });
            });
            if self.github.nothing_saved {
                ui.add_space(4.0);
                ui.label(RichText::new(tr("No saved GitHub login was found. Sign in in the browser, or use a token.")).color(pal.warning).size(11.5));
            }
            ui.add_space(8.0);
            // ---- more ways
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 8.0, r.center().y), Vec2::splat(14.0)), if more { Icon::ChevronDown } else { Icon::ChevronRight }, pal.text_dim);
            ui.painter().text(pos2(r.min.x + 24.0, r.center().y), Align2::LEFT_CENTER, tr("Token or public repositories"), fonts::ui(12.5), if resp.hovered() { pal.text } else { pal.text_dim });
            if resp.clicked() {
                self.github.more = !more;
            }
            if more {
                ui.add_space(4.0);
                ui.label(RichText::new(tr("Personal access token")).font(fonts::semibold(13.0)));
                ui.add_space(4.0);
                widgets::right_row(ui, widgets::FIELD_H, |ui| {
                    ui.add_enabled_ui(!busy && !self.github.token_input.trim().is_empty(), |ui| {
                        if widgets::primary_button(ui, &pal, tr("Sign in")).clicked() {
                            token_in = Some((self.github.token_input.clone(), self.github.remember));
                        }
                    });
                    let r = TextField::new(&mut self.github.token_input).hint("ghp_...").icon(Icon::Key).password(true).enabled(!busy).show(ui, &pal);
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.github.token_input.trim().is_empty() {
                        token_in = Some((self.github.token_input.clone(), self.github.remember));
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    widgets::checkbox(ui, &pal, &mut self.github.remember, tr("Remember in Git's credential store"));
                    widgets::right_row(ui, 28.0, |ui| {
                        if widgets::secondary_button(ui, &pal, tr("Create a token on GitHub")).on_hover_text(tr("Opens github.com in the browser; tick the 'repo' scope to also see private repositories")).clicked() {
                            token_page = true;
                        }
                    });
                });
                ui.add_space(10.0);
                ui.label(RichText::new(tr("Public repositories of a user")).font(fonts::semibold(13.0)));
                ui.add_space(4.0);
                widgets::right_row(ui, widgets::FIELD_H, |ui| {
                    ui.add_enabled_ui(!busy && !self.github.user_input.trim().is_empty(), |ui| {
                        if widgets::secondary_button(ui, &pal, tr("Show")).clicked() {
                            user_in = Some(self.github.user_input.clone());
                        }
                    });
                    let r = TextField::new(&mut self.github.user_input).hint(tr("GitHub username")).icon(Icon::User).enabled(!busy).show(ui, &pal);
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.github.user_input.trim().is_empty() {
                        user_in = Some(self.github.user_input.clone());
                    }
                });
            }
            if let Some(e) = &self.github.error {
                ui.add_space(8.0);
                ui.label(RichText::new(error_text(e)).color(pal.error).size(12.0));
            }
        });
        if browser_login {
            self.github_sign_in_browser();
        }
        if cancel_login {
            self.github_cancel_browser();
        }
        if saved {
            self.github_sign_in_saved();
        }
        if let Some((t, remember)) = token_in {
            self.github_sign_in_token(&t, remember);
        }
        if let Some(u) = user_in {
            self.github_show_user(&u);
        }
        if token_page {
            crate::input::open_url(github::TOKEN_URL);
        }
    }

    // ------------------------------------------------------------------ the list

    fn draw_github_list(&mut self, ui: &mut egui::Ui, st: &mut CloneState, running: bool) -> bool {
        let pal = self.pal;
        let mut clone_now = false;
        let mut refresh = false;
        let mut sign_out = false;
        let mut back = false;
        let own = self.github.listing == Some(Listing::Mine);
        let who = self.github.shown.clone();
        let total = self.github.repos.len();
        // ---- header: avatar, name, counts, actions
        ui.horizontal(|ui| {
            ui.set_min_height(36.0);
            ui.spacing_mut().item_spacing.x = 8.0;
            let login = who.as_ref().map(|a| a.login.clone()).unwrap_or_else(|| match &self.github.listing {
                Some(Listing::User(u)) => u.clone(),
                _ => String::new(),
            });
            avatar(ui, &pal, &login, 32.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(&login).font(fonts::semibold(14.5)));
                let sub = match (&who, own) {
                    (Some(a), true) => a.name.clone().unwrap_or_else(|| tr("Signed in to GitHub").to_string()),
                    (Some(a), false) => a.name.clone().unwrap_or_else(|| tr("Public repositories").to_string()),
                    (None, _) => String::new(),
                };
                ui.label(RichText::new(sub).color(pal.text_dim).size(11.5));
            });
            if self.github.loading {
                ui.add(egui::Spinner::new().size(16.0).color(pal.accent));
                ui.label(RichText::new(i18n::fmt("Loading... {}", &[&total])).color(pal.text_dim).size(12.0));
            } else {
                ui.label(RichText::new(i18n::fmt("{} repositories", &[&total])).color(pal.text_dim).size(12.0));
            }
            widgets::right_row(ui, 32.0, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if own {
                    if widgets::secondary_button(ui, &pal, tr("Sign out")).clicked() {
                        sign_out = true;
                    }
                } else if widgets::secondary_button(ui, &pal, tr("Back")).clicked() {
                    back = true;
                }
                if widgets::icon_button(ui, &pal, Icon::Refresh, 30.0, pal.text_dim, tr("Reload the list")).clicked() {
                    refresh = true;
                }
            });
        });
        ui.add_space(8.0);
        // ---- search and toggles
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 250.0).max(160.0);
            TextField::new(&mut self.github.filter).hint(tr("Search repositories")).icon(Icon::Search).width(w).height(34.0).show(ui, &pal);
            widgets::checkbox(ui, &pal, &mut self.github.include_forks, tr("Forks"));
            widgets::checkbox(ui, &pal, &mut self.github.include_archived, tr("Archived"));
        });
        ui.add_space(8.0);

        // ---- rows
        let visible = self.github.visible();
        let list_h = (ui.ctx().content_rect().height() - 560.0).clamp(90.0, 300.0);
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), list_h), Sense::hover());
        ui.painter().rect_filled(area, CornerRadius::same(14), pal.hover);
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink2(vec2(4.0, 6.0))).layout(egui::Layout::top_down(egui::Align::Min)));
        self.github.rects.clear();
        let mut rects: Vec<(String, Rect)> = vec![];
        let mut pick: Option<usize> = None;
        let mut double: Option<usize> = None;
        if visible.is_empty() {
            inner.add_space(30.0);
            let (text, icon, color) = if let Some(e) = &self.github.error {
                (error_text(e), Icon::Warning, pal.error)
            } else if self.github.loading {
                (tr("Loading repositories...").to_string(), Icon::Spinner, pal.text_dim)
            } else if total == 0 {
                (tr("No repositories").to_string(), Icon::Repo, pal.text_dim)
            } else {
                (tr("No repository matches the search").to_string(), Icon::Search, pal.text_dim)
            };
            inner.vertical_centered(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
                icons::paint(ui.painter(), r, icon, color);
                ui.add_space(6.0);
                ui.label(RichText::new(text).color(color).size(12.5));
            });
        }
        let selected = st.gh_selected.clone();
        egui::ScrollArea::vertical().id_salt("github_repos").auto_shrink([false, false]).show_rows_fading(&mut inner, ROW_H, visible.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for k in range {
                let i = visible[k];
                let repo: &Repo = &self.github.repos[i];
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let is_sel = selected.as_deref() == Some(repo.full_name.as_str());
                widgets::row_background(ui, &pal, r, is_sel, resp.hovered());
                rects.push((repo.full_name.clone(), r));
                let clip = ui.painter().with_clip_rect(r.shrink2(vec2(8.0, 0.0)));
                icons::paint(&clip, Rect::from_center_size(pos2(r.min.x + 28.0, r.center().y), Vec2::splat(18.0)), if repo.private { Icon::Lock } else { Icon::Repo }, if repo.private { pal.warning } else { pal.text_dim });
                // name line
                let y1 = r.min.y + 18.0;
                let owner = format!("{}/", repo.owner);
                let go = clip.layout_no_wrap(owner, fonts::ui(13.5), pal.text_dim);
                let gn = clip.layout_no_wrap(repo.name.clone(), fonts::semibold(13.5), pal.text);
                let (ow, nw) = (go.size().x, gn.size().x);
                clip.galley(pos2(r.min.x + 48.0, y1 - go.size().y / 2.0), go, pal.text_dim);
                clip.galley(pos2(r.min.x + 48.0 + ow, y1 - gn.size().y / 2.0), gn, pal.text);
                let mut bx = r.min.x + 56.0 + ow + nw;
                if repo.private {
                    bx += badge(ui, &pal, bx, y1, tr("private"), pal.warning);
                }
                if repo.fork {
                    bx += badge(ui, &pal, bx, y1, tr("fork"), pal.text_dim);
                }
                if repo.archived {
                    badge(ui, &pal, bx, y1, tr("archived"), pal.text_dim);
                }
                // description line
                let desc = if repo.description.is_empty() { tr("No description").to_string() } else { crate::ui_chrome::elide(&repo.description, 80) };
                clip.text(pos2(r.min.x + 48.0, r.min.y + 38.0), Align2::LEFT_CENTER, desc, fonts::ui(12.0), pal.text_dim);
                // right side: language, stars, age
                let mut rx = r.max.x - 16.0;
                let age = age_text(&repo.pushed_at);
                if !age.is_empty() {
                    clip.text(pos2(rx, y1), Align2::RIGHT_CENTER, &age, fonts::ui(11.5), pal.text_dim);
                }
                if let Some(lang) = &repo.language {
                    clip.text(pos2(rx, r.min.y + 38.0), Align2::RIGHT_CENTER, lang, fonts::ui(11.5), pal.text_dim);
                    rx -= clip.layout_no_wrap(lang.clone(), fonts::ui(11.5), pal.text_dim).size().x + 14.0;
                }
                if repo.stars > 0 {
                    clip.text(pos2(rx, r.min.y + 38.0), Align2::RIGHT_CENTER, format!("\u{2605} {}", repo.stars), fonts::ui(11.5), pal.text_dim);
                }
                if resp.double_clicked() {
                    double = Some(i);
                } else if resp.clicked() {
                    pick = Some(i);
                }
            }
        });
        self.github.rects = rects;
        if let Some(i) = double.or(pick) {
            let repo = self.github.repos[i].clone();
            st.url = repo.clone_url.clone();
            st.gh_selected = Some(repo.full_name);
            st.error = None;
            clone_now = double.is_some() && !running;
        }
        if let Some(sel) = st.gh_selected.as_ref()
            && !self.github.repos.iter().any(|r| &r.full_name == sel)
            && !self.github.loading
        {
            st.gh_selected = None;
        }
        if sign_out {
            self.github_sign_out();
            st.gh_selected = None;
        }
        if back {
            self.github.listing = None;
            self.github.repos.clear();
            self.github.shown = None;
            self.github.error = None;
            st.gh_selected = None;
        }
        if refresh {
            self.github_refresh();
        }
        clone_now
    }
}

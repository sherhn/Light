//! The Services window: what the project runs - the programs of the Run tabs that are still going, and the containers
//! of Docker grouped by their Compose project, each started, stopped, restarted or removed with a click and with its
//! logs followed live. Docker is the Docker module's: the window shows it while the module is on.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Align, Layout, Pos2, Rect, RichText, Sense, Vec2};
use ide_core::containers::{self, Container};
use lisp_runtime::runner::{self, RunEvent, RunHandle};

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::state::*;
use crate::widgets::{self, ROW_H};

/// How often the list of containers is asked again while the window is shown.
const REFRESH_SECS: f64 = 3.0;

struct Logs {
    id: String,
    handle: Option<RunHandle>,
    lines: Vec<LogLine>,
}

#[derive(Default)]
pub struct ServicesState {
    pub containers: Vec<Container>,
    /// The containers have been listed at least once.
    pub listed: bool,
    pub error: Option<String>,
    loading: Option<Receiver<Result<Vec<Container>, String>>>,
    refreshed_at: f64,
    pub selected: Option<String>,
    logs: Option<Logs>,
    pub(crate) action: Option<Receiver<(String, Result<String, String>)>>,
    pub busy: Option<String>,
    /// Where the rows and buttons were drawn (the UI tests click them).
    pub rows: Vec<(String, Rect)>,
}

impl ServicesState {
    /// Ask Docker again on the next frame.
    pub fn refreshed_at_reset(&mut self) {
        self.refreshed_at = -100.0;
    }

    /// Nothing is being asked or done: the list is what Docker says now.
    pub fn idle(&self) -> bool {
        self.listed && self.loading.is_none() && self.action.is_none()
    }
}

/// `docker ...` by the shell of this computer (it finds `docker.exe` and the scripts that stand for it alike).
fn docker(args: &str, cwd: &std::path::Path) -> std::process::Command {
    let mut cmd = ide_core::cmdscript::shell_command(&format!("docker {args}"), cwd, &[]);
    lisp_runtime::no_window(&mut cmd);
    cmd
}

impl App {
    /// Is the Docker module on (so the window shows containers)?
    pub fn docker_enabled(&self) -> bool {
        self.mods.installed.iter().any(|m| m.enabled && m.manifest.id == "docker")
    }

    fn docker_cwd(&self) -> PathBuf {
        self.project_root().map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
    }

    pub fn refresh_containers(&mut self) {
        if self.services.loading.is_some() || !self.docker_enabled() {
            return;
        }
        let cwd = self.docker_cwd();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = docker("ps --all --no-trunc --format \"{{json .}}\"", &cwd).output().map_err(|e| e.to_string()).and_then(|out| {
                if out.status.success() {
                    Ok(containers::parse_ps(&String::from_utf8_lossy(&out.stdout)))
                } else {
                    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    Err(if err.is_empty() { "docker failed".into() } else { err })
                }
            });
            let _ = tx.send(result);
        });
        self.services.loading = Some(rx);
        self.services.refreshed_at = self.now;
    }

    /// Every frame: the list, the action that ran, the logs.
    pub fn poll_services(&mut self) {
        let shown = self.bottom_tool == Some(BottomTool::Services);
        if shown && self.docker_enabled() && self.now - self.services.refreshed_at > REFRESH_SECS {
            self.refresh_containers();
        }
        if let Some(rx) = &self.services.loading
            && let Ok(result) = rx.try_recv()
        {
            self.services.loading = None;
            self.services.listed = true;
            match result {
                Ok(list) => {
                    self.services.containers = list;
                    self.services.error = None;
                }
                Err(e) => self.services.error = Some(e.lines().next().unwrap_or_default().to_string()),
            }
        }
        if let Some(rx) = &self.services.action
            && let Ok((what, result)) = rx.try_recv()
        {
            self.services.action = None;
            self.services.busy = None;
            if let Err(e) = result {
                self.toast(i18n::fmt("{0} failed: {1}", &[&what, &e.lines().next().unwrap_or_default()]), Sev::Error);
            }
            self.services.refreshed_at = -100.0;
            self.refresh_containers();
            // a container that started again has new logs to follow
            if let Some(id) = self.services.selected.clone() {
                self.follow_logs(&id);
            }
        }
        if let Some(logs) = self.services.logs.as_mut()
            && let Some(h) = &logs.handle
        {
            let mut ended = false;
            while let Some(ev) = h.try_recv() {
                match ev {
                    RunEvent::Output { text, is_err } => {
                        for line in text.replace('\r', "").split_inclusive('\n') {
                            let kind = if is_err { LogKind::Error } else { LogKind::Output };
                            match logs.lines.last_mut() {
                                Some(l) if l.kind == kind && !l.text.ends_with('\n') => l.text.push_str(line),
                                _ => logs.lines.push(LogLine { kind, text: line.to_string() }),
                            }
                        }
                    }
                    RunEvent::Exited(_) => ended = true,
                }
            }
            if logs.lines.len() > 5000 {
                logs.lines.drain(..1000);
            }
            if ended {
                logs.handle = None;
            }
        }
    }

    /// Show the logs of container `id`, following them while it runs.
    pub fn follow_logs(&mut self, id: &str) {
        if let Some(h) = self.services.logs.take().and_then(|l| l.handle) {
            h.kill();
        }
        if !containers::plain_name(id) {
            return;
        }
        let handle = runner::spawn_streaming(docker(&format!("logs --follow --tail 300 {id}"), &self.docker_cwd())).ok();
        self.services.logs = Some(Logs { id: id.to_string(), handle, lines: vec![] });
    }

    pub fn select_container(&mut self, id: &str) {
        self.services.selected = Some(id.to_string());
        self.follow_logs(id);
    }

    /// `start`, `stop`, `restart` or `rm -f` a container, in the background.
    pub fn container_action(&mut self, verb: &str, c: &Container) {
        if self.services.action.is_some() || !containers::plain_name(&c.id) {
            return;
        }
        let what = match verb { "start" => tr("Start"), "stop" => tr("Stop"), "restart" => tr("Restart"), _ => tr("Remove") }.to_string();
        let args = if verb == "rm" { format!("rm --force {}", c.id) } else { format!("{verb} {}", c.id) };
        let cwd = self.docker_cwd();
        let (tx, rx) = mpsc::channel();
        let label = what.clone();
        std::thread::spawn(move || {
            let result = docker(&args, &cwd).output().map_err(|e| e.to_string()).and_then(|out| if out.status.success() { Ok(String::new()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) });
            let _ = tx.send((label, result));
        });
        self.services.action = Some(rx);
        self.services.busy = Some(format!("{what}: {}", c.name));
        if verb == "rm" && self.services.selected.as_deref() == Some(c.id.as_str()) {
            self.services.selected = None;
            if let Some(h) = self.services.logs.take().and_then(|l| l.handle) {
                h.kill();
            }
        }
    }

    /// What the logs of the container shown say (for the UI tests).
    pub fn container_log_text(&self) -> String {
        self.services.logs.as_ref().map(|l| l.lines.iter().map(|x| x.text.as_str()).collect()).unwrap_or_default()
    }

    pub fn stop_services(&mut self) {
        self.services.refreshed_at_reset();
        if let Some(h) = self.services.logs.take().and_then(|l| l.handle) {
            h.kill();
        }
    }

    /// The Services window: on the left what runs (the Run tabs still going, the containers by project), on the right
    /// the container chosen - its state, its buttons and its logs.
    pub fn draw_services(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let runs: Vec<(String, String)> = self.run_tabs().into_iter().filter(|(_, _, s)| *s == crate::app_run::RunStatus::Running).map(|(k, t, _)| (k, t)).collect();
        let groups = containers::grouped(&self.services.containers);
        let docker = self.docker_enabled();
        let mut rows = Vec::new();
        let mut select = None;
        let mut show_run = None;
        let mut stop_run = None;
        let mut action: Option<(&'static str, Container)> = None;
        let mut refresh = false;
        let avail = ui.available_size();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(Vec2::new((avail.x * 0.42).max(260.0), avail.y), Layout::top_down(Align::Min), |ui| {
                egui::ScrollArea::vertical().id_salt("services_list").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // the programs of the Run window that are still going
                    self.services_header(ui, Icon::Play, tr("Running"), &i18n::fmt("{}", &[&runs.len()]), None);
                    if runs.is_empty() {
                        ui.label(RichText::new(format!("      {}", tr("Nothing runs now"))).color(pal.text_dim).size(12.0));
                    }
                    for (key, title) in &runs {
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 2.0), Sense::click());
                        widgets::row_background(ui, &pal, r, false, resp.hovered());
                        ui.painter().circle_filled(Pos2::new(r.min.x + 30.0, r.center().y), 4.0, pal.ok);
                        ui.painter().text(Pos2::new(r.min.x + 44.0, r.center().y), egui::Align2::LEFT_CENTER, title, fonts::ui(13.0), pal.text);
                        let stop = Rect::from_center_size(Pos2::new(r.max.x - 22.0, r.center().y), Vec2::splat(22.0));
                        let sr = ui.interact(stop, ui.scope_id().with(("stop_run", key)), Sense::click());
                        icons::paint(ui.painter(), stop.shrink(4.0), Icon::Stop, if sr.hovered() { pal.error } else { pal.text_dim });
                        if sr.on_hover_text(tr("Stop")).clicked() {
                            stop_run = Some(key.clone());
                        } else if resp.clicked() {
                            show_run = Some(key.clone());
                        }
                    }
                    ui.add_space(6.0);
                    if !docker {
                        return;
                    }
                    let running = self.services.containers.iter().filter(|c| c.running()).count();
                    if self.services_header(ui, Icon::Package, "Docker", &i18n::fmt("{0} of {1} running", &[&running, &self.services.containers.len()]), Some(&pal)) {
                        refresh = true;
                    }
                    if let Some(e) = &self.services.error {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(28.0);
                            ui.label(RichText::new(i18n::fmt("Docker does not answer: {}", &[e])).color(pal.error).size(12.0));
                        });
                    } else if !self.services.listed {
                        ui.label(RichText::new(format!("      {}", tr("Asking Docker..."))).color(pal.text_dim).size(12.0));
                    } else if groups.is_empty() {
                        ui.label(RichText::new(format!("      {}", tr("No containers"))).color(pal.text_dim).size(12.0));
                    }
                    for (project, list) in &groups {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 6.0), Sense::hover());
                        icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(r.min.x + 24.0, r.center().y - 7.0), Vec2::splat(14.0)), Icon::Folder, pal.builtin);
                        let name = if project.is_empty() { tr("On their own").to_string() } else { project.clone() };
                        ui.painter().text(Pos2::new(r.min.x + 44.0, r.center().y), egui::Align2::LEFT_CENTER, name, fonts::semibold(12.5), pal.text);
                        for c in list {
                            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                            let selected = self.services.selected.as_deref() == Some(c.id.as_str());
                            widgets::row_background(ui, &pal, r, selected, resp.hovered());
                            let dot = match c.state.as_str() { "running" => pal.ok, "paused" | "restarting" => pal.warning, "dead" => pal.error, _ => pal.text_dim.gamma_multiply(0.6) };
                            ui.painter().circle_filled(Pos2::new(r.min.x + 50.0, r.center().y), 4.0, dot);
                            let label = if c.service.is_empty() { c.name.clone() } else { c.service.clone() };
                            let end = ui.painter().text(Pos2::new(r.min.x + 62.0, r.center().y), egui::Align2::LEFT_CENTER, &label, fonts::ui(13.5), pal.text).max.x;
                            ui.painter().with_clip_rect(Rect::from_min_max(Pos2::new(end, r.min.y), Pos2::new(r.max.x - 8.0, r.max.y))).text(Pos2::new(end + 10.0, r.center().y), egui::Align2::LEFT_CENTER, &c.image, fonts::ui(12.0), pal.text_dim);
                            rows.push((format!("container:{}", c.name), r));
                            if resp.on_hover_text(format!("{}\n{}", c.name, c.status)).clicked() {
                                select = Some(c.id.clone());
                            }
                        }
                    }
                });
            });
            ui.separator();
            // the container chosen
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width() - 8.0, avail.y), Layout::top_down(Align::Min), |ui| {
                let Some(c) = self.services.selected.as_ref().and_then(|id| self.services.containers.iter().find(|c| c.id == *id)).cloned() else {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(if docker { tr("Choose a container to see its logs and to start or stop it") } else { tr("Turn the Docker module on to see its containers here") }).color(pal.text_dim));
                    });
                    return;
                };
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new(&c.name).font(fonts::semibold(14.0)));
                    ui.label(RichText::new(&c.status).color(if c.running() { pal.ok } else { pal.text_dim }).size(12.5));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(6.0);
                        ui.add_enabled_ui(self.services.action.is_none(), |ui| {
                            let buttons: Vec<(&'static str, &str, bool)> = if c.running() { vec![("rm", tr("Remove"), false), ("restart", tr("Restart"), false), ("stop", tr("Stop"), true)] } else { vec![("rm", tr("Remove"), false), ("start", tr("Start"), true)] };
                            for (verb, label, primary) in buttons {
                                let b = widgets::compact_button(ui, &pal, label, primary);
                                rows.push((format!("action:{verb}"), b.rect));
                                if b.clicked() {
                                    action = Some((verb, c.clone()));
                                }
                            }
                        });
                        if let Some(b) = &self.services.busy {
                            ui.label(RichText::new(b).color(pal.text_dim).size(12.0));
                        }
                    });
                });
                if !c.ports.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new(i18n::fmt("Ports: {}", &[&c.ports])).color(pal.text_dim).size(12.0));
                    });
                }
                ui.add_space(4.0);
                let lines = self.services.logs.as_ref().filter(|l| l.id == c.id).map(|l| l.lines.clone()).unwrap_or_default();
                if lines.is_empty() {
                    ui.label(RichText::new(format!("  {}", tr("No logs yet"))).color(pal.text_dim).size(12.0));
                }
                let _ = self.draw_log(ui, "container_logs", &lines, true, "");
            });
        });
        self.services.rows = rows;
        if refresh {
            self.services.refreshed_at = -100.0;
            self.refresh_containers();
        }
        if let Some(id) = select {
            self.select_container(&id);
        }
        if let Some(key) = show_run {
            self.bottom_tool = Some(BottomTool::Run);
            self.show_run_tab(&key);
        }
        if let Some(key) = stop_run {
            self.show_run_tab(&key);
            self.stop_run();
        }
        if let Some((verb, c)) = action {
            if verb == "rm" {
                self.popup = Popup::Confirm { kind: ConfirmKind::RemoveContainer(c.id.clone()), message: i18n::fmt("Remove the container '{}'? Its own files go with it; its image and volumes stay.", &[&c.name]) };
            } else {
                self.container_action(verb, &c);
            }
        }
    }

    /// A group's heading in the list; with `refresh` a button that asks again (returns whether it was clicked).
    fn services_header(&mut self, ui: &mut egui::Ui, icon: Icon, title: &str, detail: &str, refresh: Option<&crate::theme::Palette>) -> bool {
        let pal = self.pal;
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
        icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(r.min.x + 8.0, r.center().y - 7.0), Vec2::splat(14.0)), icon, pal.accent);
        let end = ui.painter().text(Pos2::new(r.min.x + 28.0, r.center().y), egui::Align2::LEFT_CENTER, title, fonts::semibold(13.0), pal.text).max.x;
        ui.painter().text(Pos2::new(end + 8.0, r.center().y), egui::Align2::LEFT_CENTER, detail, fonts::ui(12.0), pal.text_dim);
        if refresh.is_some() {
            let b = Rect::from_center_size(Pos2::new(r.max.x - 22.0, r.center().y), Vec2::splat(22.0));
            let resp = ui.interact(b, ui.scope_id().with(("services_refresh", title)), Sense::click());
            icons::paint(ui.painter(), b.shrink(4.0), Icon::Refresh, if resp.hovered() { pal.text } else { pal.text_dim });
            return resp.on_hover_text(tr("Refresh")).clicked();
        }
        false
    }
}

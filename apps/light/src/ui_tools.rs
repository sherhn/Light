//! The developer tools on the Linux page: every group the modules and the user ask for, with a box to take it or leave
//! it, its packages one by one, and a form for adding a tool of one's own.

use eframe::egui::{self, Margin, RichText, vec2};
use ide_core::changes::Check;
use lisp_runtime::wsl::{self, ToolGroup};

use crate::app::App;
use crate::app_wsl::{ToolDraft, ToolsFound};
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::Icon;
use crate::ui_settings::Rows;
use crate::ui_wsl::{spinner, status_ok};
use crate::widgets::{self, TextField};

impl App {
    /// The rows of the developer tools of `selected` (what the check `tools` found there). Returns whether the user
    /// asked to install what is chosen.
    pub(crate) fn draw_tool_rows(&mut self, ui: &mut egui::Ui, rows: &mut Rows, selected: &str, busy_task: bool, tools: &Option<ToolsFound>, checking: bool) -> bool {
        let pal = self.pal;
        let mut install = false;
        let groups = self.tool_groups();
        let chosen = self.chosen_tool_groups(&[]);
        let found: Vec<(String, bool)> = tools.as_ref().and_then(|r| r.as_ref().ok()).cloned().unwrap_or_default();
        let is_there = |p: &str| found.iter().any(|(n, ok)| *ok && n == p);

        // what is wanted: the programs of the groups that have something chosen, and how many packages that is
        let mut wanted: Vec<&str> = Vec::new();
        let mut packages_chosen: Vec<&str> = Vec::new();
        for g in &chosen {
            for p in g.programs.iter().map(String::as_str).filter(|p| !wanted.contains(p)).collect::<Vec<_>>() {
                wanted.push(p);
            }
            for p in g.package_list() {
                if !packages_chosen.contains(&p) {
                    packages_chosen.push(p);
                }
            }
        }
        let mut packages_all: Vec<&str> = Vec::new();
        for p in groups.iter().flat_map(|g| g.package_list()) {
            if !packages_all.contains(&p) {
                packages_all.push(p);
            }
        }
        let missing = wanted.iter().filter(|p| !is_there(p)).count();
        let n_chosen = packages_chosen.len();

        let desc = match tools {
            _ if checking => i18n::fmt("Looking for the programs below in {}...", &[&selected]),
            Some(Ok(_)) if n_chosen == 0 => tr("No tool is chosen: tick the ones to install").to_string(),
            Some(Ok(_)) if missing == 0 => tr("Everything that is chosen is installed.").to_string(),
            Some(Ok(_)) => i18n::fmt("{0} of {1} programs are missing. Ubuntu and Debian get every package ticked below; other distributions the C toolchain, nasm, gdb, git and QEMU, and the tools you added.", &[&missing, &wanted.len()]),
            Some(Err(e)) => i18n::fmt("Could not check {0}: {1}", &[&selected, &crate::ui_chrome::elide(e, 140)]),
            None => tr("Not checked yet.").to_string(),
        };
        rows.row(ui, &pal, tr("Compilers, debuggers and everyday tools"), &desc, 250.0, |ui| {
            if checking {
                spinner(ui, &pal);
            } else if matches!(tools, Some(Ok(_))) && missing == 0 && n_chosen > 0 {
                status_ok(ui, &pal, tr("Ready"));
            } else {
                ui.add_enabled_ui(!busy_task && n_chosen > 0, |ui| {
                    if widgets::primary_button(ui, &pal, tr("Install the chosen tools")).clicked() {
                        install = true;
                    }
                });
            }
        });

        // all or none of the packages at once
        let (mut all, mut none) = (false, false);
        rows.row(ui, &pal, tr("Packages to install"), &i18n::fmt("{0} of {1} packages are ticked", &[&n_chosen, &packages_all.len()]), 240.0, |ui| {
            if widgets::secondary_button(ui, &pal, tr("Untick all")).clicked() {
                none = true;
            }
            if widgets::secondary_button(ui, &pal, tr("Tick all")).clicked() {
                all = true;
            }
        });
        if all || none {
            for p in &packages_all {
                self.settings.linux_tools.choose(p, all);
            }
            self.save_settings();
        }

        // the groups, each with its packages to choose from
        for g in &groups {
            self.draw_tool_group(ui, rows, g, &found);
        }

        // a tool of one's own
        let mut open_form = false;
        let editing = self.wsl.tool_draft.is_some();
        rows.row(ui, &pal, tr("Your own tools"), tr("Any packages of the distribution, named for its package manager (apt on Ubuntu and Debian): nmap, libssl-dev, ..."), 170.0, |ui| {
            ui.add_enabled_ui(!editing, |ui| {
                if widgets::secondary_button(ui, &pal, tr("Add a tool...")).clicked() {
                    open_form = true;
                }
            });
        });
        if open_form {
            self.wsl.tool_draft = Some(ToolDraft::default());
        }
        let (mut save, mut cancel) = (false, false);
        if let Some(d) = self.wsl.tool_draft.as_mut() {
            ui.add_space(4.0);
            ui.label(RichText::new(if d.index.is_some() { tr("Change the tool") } else { tr("Add a tool of your own") }).font(fonts::semibold(13.0)));
            ui.add_space(8.0);
            ui.label(RichText::new(tr("Name")).color(pal.text_dim).size(12.0));
            ui.add_space(4.0);
            TextField::new(&mut d.name).hint(tr("for example Networking")).icon(Icon::Package).show(ui, &pal);
            ui.add_space(8.0);
            ui.label(RichText::new(tr("Packages, separated by spaces")).color(pal.text_dim).size(12.0));
            ui.add_space(4.0);
            TextField::new(&mut d.packages).hint(tr("for example nmap tcpdump")).icon(Icon::Install).show(ui, &pal);
            ui.add_space(8.0);
            ui.label(RichText::new(tr("Programs that show it is installed (optional)")).color(pal.text_dim).size(12.0));
            ui.add_space(4.0);
            TextField::new(&mut d.programs).hint(tr("empty: the packages themselves are looked for")).icon(Icon::Terminal).show(ui, &pal);
            if !d.error.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&d.error).color(pal.error).size(12.0));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &pal, tr("Save")).clicked() {
                    save = true;
                }
                if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                    cancel = true;
                }
            });
            ui.add_space(14.0);
        }
        if save {
            self.wsl_tool_save();
        }
        if cancel {
            self.wsl.tool_draft = None;
        }
        install
    }

    /// One group of tools: the box, its programs, how much of it is there, and (opened) its packages.
    fn draw_tool_group(&mut self, ui: &mut egui::Ui, rows: &mut Rows, g: &ToolGroup, found: &[(String, bool)]) {
        let pal = self.pal;
        let pkgs = g.package_list();
        let n_on = pkgs.iter().filter(|p| self.settings.linux_tools.is_chosen(p)).count();
        let state = if n_on == pkgs.len() {
            Check::On
        } else if n_on == 0 {
            Check::Off
        } else {
            Check::Mixed
        };
        let absent = g.programs.iter().filter(|p| !found.iter().any(|(n, ok)| *ok && n == *p)).count();
        let mut desc = g.programs.iter().map(|p| wsl::check_label(p)).collect::<Vec<_>>().join(", ");
        if state == Check::Mixed {
            desc.push_str(&format!("  ·  {}", i18n::fmt("{0} of {1} packages", &[&n_on, &pkgs.len()])));
        }
        let open = self.wsl.tools_open.contains(&g.name);
        let custom = if g.custom { self.settings.linux_tools.custom.iter().position(|c| c.name.trim() == g.name) } else { None };
        let (mut toggle_open, mut edit, mut remove) = (false, false, false);
        let clicked = rows.check_row(ui, &pal, state, &i18n::tr_dyn(&g.name), &desc, 250.0, |ui| {
            if widgets::icon_button(ui, &pal, if open { Icon::ChevronUp } else { Icon::ChevronDown }, 30.0, pal.text_dim, tr("Choose packages")).clicked() {
                toggle_open = true;
            }
            if custom.is_some() {
                if widgets::icon_button(ui, &pal, Icon::Trash, 30.0, pal.text_dim, tr("Remove this tool")).clicked() {
                    remove = true;
                }
                if widgets::icon_button(ui, &pal, Icon::Pencil, 30.0, pal.text_dim, tr("Change this tool")).clicked() {
                    edit = true;
                }
            }
            if n_on == 0 {
                ui.label(RichText::new(tr("Skipped")).color(pal.text_dim).font(fonts::semibold(12.5)));
            } else if found.is_empty() {
                // not looked for yet
            } else if absent == 0 {
                status_ok(ui, &pal, tr("Ready"));
            } else {
                ui.label(RichText::new(i18n::fmt("{} missing", &[&absent])).color(pal.warning).font(fonts::semibold(12.5)));
            }
        });
        if clicked {
            let on = state != Check::On;
            for p in &pkgs {
                self.settings.linux_tools.choose(p, on);
            }
            self.save_settings();
        }
        if toggle_open && !self.wsl.tools_open.remove(&g.name) {
            self.wsl.tools_open.insert(g.name.clone());
        }
        if let Some(i) = custom {
            if edit {
                let c = self.settings.linux_tools.custom[i].clone();
                self.wsl.tool_draft = Some(ToolDraft { index: Some(i), name: c.name, packages: c.packages, programs: c.programs, error: String::new() });
            }
            if remove {
                self.settings.linux_tools.custom.remove(i);
                self.wsl.tools_open.remove(&g.name);
                self.wsl.tool_draft = None;
                self.wsl_tools_changed();
            }
        }
        if open {
            let mut choices: Vec<(&str, bool)> = Vec::new();
            egui::Frame::new().inner_margin(Margin { left: 32, right: 0, top: 0, bottom: 14 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                ui.horizontal_wrapped(|ui| {
                    for p in &pkgs {
                        let mut on = self.settings.linux_tools.is_chosen(p);
                        if widgets::toggle_chip(ui, &pal, p, &mut on, "").clicked() {
                            choices.push((*p, on));
                        }
                    }
                });
            });
            if !choices.is_empty() {
                for (p, on) in choices {
                    self.settings.linux_tools.choose(p, on);
                }
                self.save_settings();
            }
        }
    }
}

//! The single command registry. Every menu item, toolbar button, popup entry and key binding
//! refers to a command here by id (the same idea as PhotoCraft's command engine).

use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    File,
    Edit,
    View,
    Navigate,
    Code,
    Refactor,
    Git,
    Run,
    Tools,
    Window,
    Help,
}

impl Category {
    pub fn title(self) -> &'static str {
        match self {
            Category::File => "File",
            Category::Edit => "Edit",
            Category::View => "View",
            Category::Navigate => "Navigate",
            Category::Code => "Code",
            Category::Refactor => "Refactor",
            Category::Git => "Git",
            Category::Run => "Run",
            Category::Tools => "Tools",
            Category::Window => "Window",
            Category::Help => "Help",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    pub category: Category,
    /// Default shortcuts, e.g. `"Ctrl+Shift+A"`. `Ctrl` means the platform command key.
    pub shortcuts: &'static [&'static str],
}

macro_rules! cmds {
    ($( $id:literal, $title:literal, $cat:ident, [$($sc:literal),*]; )*) => {
        pub const COMMANDS: &[Command] = &[
            $( Command { id: $id, title: $title, category: Category::$cat, shortcuts: &[$($sc),*] }, )*
        ];
    };
}

cmds! {
    // ---- File
    "file.new", "New File...", File, ["Alt+Insert"];
    "file.new_project", "New Project...", File, [];
    "file.open_project", "Open Project...", File, ["Ctrl+Shift+O"];
    "file.clone", "Get from Version Control...", File, [];
    "file.open_file", "Open File...", File, ["Ctrl+O"];
    "file.reopen_last", "Reopen Last Project", File, [];
    "file.save", "Save", File, ["Ctrl+S"];
    "file.save_all", "Save All", File, [];
    "file.reload", "Reload from Disk", File, [];
    "file.close_tab", "Close Tab", File, ["Ctrl+F4"];
    "file.close_all", "Close All Tabs", File, ["Ctrl+Shift+F4"];
    "file.close_others", "Close Other Tabs", File, [];
    "file.next_tab", "Next Tab", File, ["Alt+Right"];
    "file.prev_tab", "Previous Tab", File, ["Alt+Left"];
    "file.reveal", "Reveal in File Manager", File, [];
    "file.copy_path", "Copy Path", File, ["Ctrl+Shift+C"];
    "app.settings", "Settings...", File, ["Ctrl+Alt+S"];
    "app.modules", "Modules...", File, ["Ctrl+Alt+P"];
    "app.exit", "Exit", File, ["Alt+F4"];
    // ---- Edit
    "edit.undo", "Undo", Edit, ["Ctrl+Z"];
    "edit.redo", "Redo", Edit, ["Ctrl+Shift+Z"];
    "edit.cut", "Cut", Edit, ["Ctrl+X", "Shift+Delete"];
    "edit.copy", "Copy", Edit, ["Ctrl+C", "Ctrl+Insert"];
    "edit.paste", "Paste", Edit, ["Ctrl+V", "Shift+Insert"];
    "edit.select_all", "Select All", Edit, ["Ctrl+A"];
    "edit.extend_selection", "Extend Selection", Edit, ["Ctrl+W"];
    "edit.shrink_selection", "Shrink Selection", Edit, ["Ctrl+Shift+W"];
    "edit.duplicate", "Duplicate Line or Selection", Edit, ["Ctrl+D"];
    "edit.delete_line", "Delete Line", Edit, ["Ctrl+Y"];
    "edit.delete_to_line_end", "Delete to Line End", Edit, ["Ctrl+Shift+Delete"];
    "edit.delete_word_back", "Delete to Word Start", Edit, ["Ctrl+Backspace"];
    "edit.delete_word_forward", "Delete to Word End", Edit, ["Ctrl+Delete"];
    "edit.move_line_up", "Move Line Up", Edit, ["Alt+Shift+Up"];
    "edit.move_line_down", "Move Line Down", Edit, ["Alt+Shift+Down"];
    "edit.join_lines", "Join Lines", Edit, ["Ctrl+Shift+J"];
    "edit.new_line_below", "Start New Line", Edit, ["Shift+Enter"];
    "edit.new_line_above", "Start New Line Before Current", Edit, ["Ctrl+Alt+Enter"];
    "edit.toggle_case", "Toggle Case", Edit, ["Ctrl+Shift+U"];
    "edit.eol_lf", "Convert Line Endings to LF", Edit, [];
    "edit.eol_crlf", "Convert Line Endings to CRLF", Edit, [];
    "edit.indent", "Indent Selection", Edit, ["Tab"];
    "edit.unindent", "Unindent Selection", Edit, ["Shift+Tab"];
    "edit.newline", "Enter", Edit, ["Enter"];
    "edit.backspace", "Backspace", Edit, ["Backspace", "Shift+Backspace"];
    "edit.delete", "Delete", Edit, ["Delete"];
    // caret movement (all part of the keymap so they can be rebound)
    "caret.left", "Move Caret Left", Edit, ["Left"];
    "caret.right", "Move Caret Right", Edit, ["Right"];
    "caret.up", "Move Caret Up", Edit, ["Up"];
    "caret.down", "Move Caret Down", Edit, ["Down"];
    "caret.word_left", "Move Caret to Previous Word", Edit, ["Ctrl+Left"];
    "caret.word_right", "Move Caret to Next Word", Edit, ["Ctrl+Right"];
    "caret.line_start", "Move Caret to Line Start", Edit, ["Home"];
    "caret.line_end", "Move Caret to Line End", Edit, ["End"];
    "caret.doc_start", "Move Caret to Text Start", Edit, ["Ctrl+Home"];
    "caret.doc_end", "Move Caret to Text End", Edit, ["Ctrl+End"];
    "caret.page_up", "Page Up", Edit, ["PageUp"];
    "caret.page_down", "Page Down", Edit, ["PageDown"];
    "select.left", "Extend Selection Left", Edit, ["Shift+Left"];
    "select.right", "Extend Selection Right", Edit, ["Shift+Right"];
    "select.up", "Extend Selection Up", Edit, ["Shift+Up"];
    "select.down", "Extend Selection Down", Edit, ["Shift+Down"];
    "select.word_left", "Extend Selection to Previous Word", Edit, ["Ctrl+Shift+Left"];
    "select.word_right", "Extend Selection to Next Word", Edit, ["Ctrl+Shift+Right"];
    "select.line_start", "Extend Selection to Line Start", Edit, ["Shift+Home"];
    "select.line_end", "Extend Selection to Line End", Edit, ["Shift+End"];
    "select.doc_start", "Extend Selection to Text Start", Edit, ["Ctrl+Shift+Home"];
    "select.doc_end", "Extend Selection to Text End", Edit, ["Ctrl+Shift+End"];
    "select.page_up", "Extend Selection Page Up", Edit, ["Shift+PageUp"];
    "select.page_down", "Extend Selection Page Down", Edit, ["Shift+PageDown"];
    "scroll.line_up", "Scroll Up", Edit, ["Ctrl+Up"];
    "scroll.line_down", "Scroll Down", Edit, ["Ctrl+Down"];
    // ---- Find
    "find.find", "Find...", Edit, ["Ctrl+F"];
    "find.replace", "Replace...", Edit, ["Ctrl+R"];
    "find.next", "Find Next", Edit, ["F3"];
    "find.prev", "Find Previous", Edit, ["Shift+F3"];
    "find.in_files", "Find in Files...", Edit, ["Ctrl+Shift+F"];
    "find.replace_in_files", "Replace in Files...", Edit, ["Ctrl+Shift+R"];
    "find.usages", "Find Usages", Edit, ["Alt+F7"];
    // ---- Navigate
    "nav.search_everywhere", "Search Everywhere", Navigate, ["Shift Shift"];
    "nav.find_action", "Find Action...", Navigate, ["Ctrl+Shift+A"];
    "nav.goto_file", "Go to File...", Navigate, ["Ctrl+Shift+N"];
    "nav.goto_symbol", "Go to Symbol...", Navigate, ["Ctrl+Alt+Shift+N"];
    "nav.goto_line", "Go to Line/Column...", Navigate, ["Ctrl+G"];
    "nav.recent_files", "Recent Files", Navigate, ["Ctrl+E"];
    "nav.declaration", "Go to Declaration", Navigate, ["Ctrl+B"];
    "nav.back", "Back", Navigate, ["Ctrl+Alt+Left"];
    "nav.forward", "Forward", Navigate, ["Ctrl+Alt+Right"];
    "nav.file_structure", "File Structure", Navigate, ["Ctrl+F12"];
    "nav.next_problem", "Next Highlighted Error", Navigate, ["F2"];
    "nav.prev_problem", "Previous Highlighted Error", Navigate, ["Shift+F2"];
    "nav.matching_bracket", "Move Caret to Matching Bracket", Navigate, ["Ctrl+Shift+M"];
    "nav.last_edit", "Last Edit Location", Navigate, ["Ctrl+Shift+Backspace"];
    // ---- Code
    "code.complete", "Basic Completion", Code, ["Ctrl+Space"];
    "code.quick_doc", "Quick Documentation", Code, ["Ctrl+Q"];
    "code.param_info", "Parameter Info", Code, ["Ctrl+P"];
    "code.reformat", "Reformat Code", Code, ["Ctrl+Alt+L"];
    "code.auto_indent", "Auto-Indent Lines", Code, ["Ctrl+Alt+I"];
    "code.comment_line", "Comment with Line Comment", Code, ["Ctrl+Slash"];
    "code.expand_macro", "Macroexpand Form", Code, ["Ctrl+Alt+M"];
    // ---- Lisp structural editing
    "lisp.slurp_forward", "Slurp Forward", Code, ["Alt+Shift+Right"];
    "lisp.barf_forward", "Barf Forward", Code, ["Alt+Shift+Left"];
    "lisp.slurp_backward", "Slurp Backward", Code, ["Ctrl+Alt+Shift+Left"];
    "lisp.barf_backward", "Barf Backward", Code, ["Ctrl+Alt+Shift+Right"];
    "lisp.raise", "Raise Form", Code, ["Alt+Shift+R"];
    "lisp.splice", "Splice Form", Code, ["Alt+Shift+S"];
    "lisp.wrap", "Wrap with Parentheses", Code, ["Alt+Shift+9"];
    "lisp.kill_sexp", "Kill Form", Code, ["Alt+Shift+K"];
    "lisp.forward_sexp", "Forward Form", Code, ["Ctrl+Alt+F"];
    "lisp.backward_sexp", "Backward Form", Code, ["Ctrl+Alt+B"];
    "lisp.up_list", "Up Form", Code, ["Ctrl+Alt+U"];
    "lisp.down_list", "Down Form", Code, ["Ctrl+Alt+D"];
    // ---- Refactor
    "refactor.rename", "Rename...", Refactor, ["Shift+F6"];
    // ---- Run / REPL
    "run.run_file", "Run Current File", Run, ["Shift+F10"];
    "run.current_file", "Run Current File", Run, ["Ctrl+Shift+F10"];
    "run.show_in_browser", "Show in browser", Run, [];
    "run.compile_file", "Compile Current File", Run, ["Ctrl+F9"];
    "run.stop", "Stop", Run, ["Ctrl+F2"];
    "repl.eval", "Send Form to REPL", Run, ["Ctrl+Enter"];
    "repl.eval_last", "Send Last Expression to REPL", Run, ["Ctrl+Shift+Enter"];
    "repl.load_file", "Load File into REPL", Run, ["Ctrl+Shift+L"];
    "repl.restart", "Restart REPL", Run, ["Ctrl+Shift+F5"];
    "repl.clear", "Clear REPL Output", Run, [];
    "repl.focus", "Focus REPL Input", Run, [];
    // ---- View / tool windows
    "view.project", "Project", View, ["Alt+1"];
    "view.run", "Run", View, ["Alt+4"];
    "view.problems", "Problems", View, ["Alt+6"];
    "view.structure", "Structure", View, ["Alt+7"];
    "view.repl", "REPL", View, ["Alt+8"];
    "view.terminal", "Terminal", View, ["Alt+F12"];
    "view.hide_all", "Hide All Tool Windows", View, ["Ctrl+Shift+F12"];
    "view.zoom_in", "Increase Font Size", View, ["Ctrl+Alt+Equals"];
    "view.zoom_out", "Decrease Font Size", View, ["Ctrl+Alt+Minus"];
    "view.zoom_reset", "Reset Font Size", View, ["Ctrl+0"];
    "view.preview_code", "Editor Only", View, [];
    "view.preview_split", "Editor and Preview", View, [];
    "view.preview_only", "Preview Only", View, [];
    "view.toggle_theme", "Toggle Dark/Light Theme", View, [];
    "view.line_numbers", "Toggle Line Numbers", View, [];
    "view.whitespace", "Toggle Whitespace", View, [];
    "view.rainbow", "Toggle Rainbow Brackets", View, [];
    // ---- Git
    "git.commit", "Commit...", Git, ["Ctrl+K"];
    "git.push", "Push", Git, ["Ctrl+Shift+K"];
    "git.pull", "Update Project (Pull)", Git, ["Ctrl+T"];
    "git.branches", "Branches...", Git, ["Ctrl+Shift+Backtick"];
    "git.new_branch", "New Branch...", Git, [];
    "git.diff", "Show Diff", Git, ["Ctrl+Alt+Shift+D"];
    "git.rollback", "Rollback Changes...", Git, ["Ctrl+Alt+Z"];
    "git.local_commit", "Local Commit...", Git, ["Ctrl+Alt+K"];
    "git.reset_base", "Compare with HEAD (Reset Diff Base)", Git, [];
    "git.refresh", "Refresh Git Status", Git, [];
    "git.init", "Create Git Repository", Git, [];
    "git.stage_file", "Add File to Git", Git, [];
    "view.git", "Git", View, ["Alt+9"];
    "view.commit", "Commit", View, ["Alt+0"];
    "view.split_right", "Split Right", View, [];
    "view.split_down", "Split Down", View, [];
    "view.unsplit", "Unsplit", View, [];
    "view.switch_pane", "Switch Pane", View, [];
    "diff.next", "Next Difference", Git, ["F7"];
    "diff.prev", "Previous Difference", Git, ["Shift+F7"];
    "diff.next_file", "Next Changed File", Git, [];
    "diff.prev_file", "Previous Changed File", Git, [];
    "run.run_config", "Choose Run Configuration...", Run, [];
    "run.scenarios", "Edit Run Scenarios...", Run, [];
    "run.load_system", "Load ASDF System into REPL", Run, [];
    "run.test_system", "Test ASDF System", Run, [];
    "edit.fold", "Fold Form", Edit, ["Ctrl+Minus"];
    "edit.unfold", "Unfold Form", Edit, ["Ctrl+Equals"];
    "edit.fold_all", "Fold All", Edit, ["Ctrl+Shift+Minus"];
    "edit.unfold_all", "Unfold All", Edit, ["Ctrl+Shift+Equals"];
    // ---- Help
    "help.keymap", "Keymap Reference", Help, [];
    "help.about", "About", Help, ["F1"];
    "app.escape", "Close Popup / Return to Editor", Window, ["Escape"];
}

/// Commands that only make sense for Lisp (its reader, its REPL, its compiler): they are hidden and off while the Lisp
/// module is.
pub fn is_lisp_command(id: &str) -> bool {
    id.starts_with("lisp.")
        || id.starts_with("repl.")
        || matches!(
            id,
            "view.repl" | "code.expand_macro" | "run.load_system" | "run.test_system" | "run.compile_file"
        )
}

pub fn by_id(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// A parsed key combination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    /// Platform command key (Ctrl on Windows/Linux, Cmd on macOS).
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: &'static str,
}

/// All key names understood by the keymap.
pub const KEY_NAMES: &[&str] = &[
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
    "Enter", "Tab", "Backspace", "Delete", "Insert", "Escape", "Space", "Left", "Right", "Up", "Down", "Home", "End", "PageUp",
    "PageDown", "Equals", "Minus", "Slash", "Comma", "Period", "Semicolon", "Backtick",
];

impl Shortcut {
    pub fn parse(s: &str) -> Option<Shortcut> {
        let mut sc = Shortcut { ctrl: false, shift: false, alt: false, key: "" };
        for part in s.split('+') {
            match part {
                "Ctrl" | "Cmd" => sc.ctrl = true,
                "Shift" => sc.shift = true,
                "Alt" => sc.alt = true,
                k => {
                    if !sc.key.is_empty() {
                        return None;
                    }
                    sc.key = KEY_NAMES.iter().copied().find(|n| *n == k)?;
                }
            }
        }
        (!sc.key.is_empty()).then_some(sc)
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        f.write_str(self.key)
    }
}

/// Pretty text for UI labels: `Ctrl+Shift+A` -> `Ctrl+Shift+A` with symbols for punctuation.
pub fn display_shortcut(s: &str) -> String {
    s.replace("Equals", "=").replace("Minus", "-").replace("Slash", "/").replace("Comma", ",").replace("Period", ".").replace("Semicolon", ";").replace("Backtick", "`")
}

pub struct Keymap {
    map: HashMap<Shortcut, &'static Command>,
}

impl Keymap {
    /// The default (IntelliJ-style) keymap derived from the registry.
    pub fn default_keymap() -> Keymap {
        let mut map = HashMap::new();
        for c in COMMANDS {
            for s in c.shortcuts {
                if let Some(sc) = Shortcut::parse(s) {
                    map.insert(sc, c);
                }
            }
        }
        Keymap { map }
    }

    pub fn lookup(&self, sc: &Shortcut) -> Option<&'static Command> {
        self.map.get(sc).copied()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lisp_commands_are_told_apart() {
        for id in ["lisp.slurp_forward", "repl.eval", "run.load_system", "code.expand_macro", "view.repl"] {
            assert!(is_lisp_command(id), "{id}");
            assert!(by_id(id).is_some(), "{id} exists");
        }
        for id in ["file.save", "edit.copy", "run.run_file", "run.stop", "find.find", "app.modules", "code.comment_line"] {
            assert!(!is_lisp_command(id), "{id}");
        }
    }
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique() {
        let mut seen = HashSet::new();
        for c in COMMANDS {
            assert!(seen.insert(c.id), "duplicate command id {}", c.id);
        }
        assert!(COMMANDS.len() > 100, "registry should be rich: {}", COMMANDS.len());
    }

    #[test]
    fn every_shortcut_parses() {
        for c in COMMANDS {
            for s in c.shortcuts {
                if *s == "Shift Shift" {
                    continue; // double-tap gesture handled by the UI
                }
                assert!(Shortcut::parse(s).is_some(), "bad shortcut {s} on {}", c.id);
            }
        }
    }

    #[test]
    fn no_conflicting_shortcuts() {
        let mut seen: HashMap<Shortcut, &str> = HashMap::new();
        for c in COMMANDS {
            for s in c.shortcuts {
                let Some(sc) = Shortcut::parse(s) else { continue };
                if let Some(prev) = seen.insert(sc, c.id) {
                    panic!("shortcut {sc} bound to both {prev} and {}", c.id);
                }
            }
        }
    }

    #[test]
    fn shortcut_roundtrip() {
        let sc = Shortcut::parse("Shift+Ctrl+Alt+F12").unwrap();
        assert_eq!(sc.to_string(), "Ctrl+Alt+Shift+F12");
        assert!(Shortcut::parse("Ctrl+").is_none());
        assert!(Shortcut::parse("Ctrl+Foo").is_none());
        assert!(Shortcut::parse("A+B").is_none());
    }

    #[test]
    fn keymap_lookup() {
        let km = Keymap::default_keymap();
        let c = km.lookup(&Shortcut::parse("Ctrl+Shift+A").unwrap()).unwrap();
        assert_eq!(c.id, "nav.find_action");
        assert_eq!(km.lookup(&Shortcut::parse("Ctrl+S").unwrap()).unwrap().id, "file.save");
        assert!(km.lookup(&Shortcut::parse("Ctrl+Shift+Q").unwrap()).is_none());
    }

    #[test]
    fn all_titles_non_empty_and_lookup_by_id() {
        for c in COMMANDS {
            assert!(!c.title.is_empty());
            assert_eq!(by_id(c.id).unwrap().id, c.id);
        }
        assert!(by_id("nope").is_none());
    }
}

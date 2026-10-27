//! The terminal's command history: one list for every project, kept between sessions. The file holds one JSON
//! string per line, so a command pasted over several lines comes back as it was typed.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// Commands kept; the oldest are dropped first.
pub const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Default)]
pub struct CommandHistory {
    file: Option<PathBuf>,
    entries: Vec<String>,
    /// Lines in the file, which also holds the entries dropped from memory since it was last rewritten.
    on_disk: usize,
}

impl CommandHistory {
    /// A history that is not saved anywhere.
    pub fn in_memory() -> CommandHistory {
        CommandHistory::default()
    }

    /// The history saved in `file` (missing or damaged lines are skipped); new commands are appended to it.
    pub fn load(file: PathBuf) -> CommandHistory {
        let mut history = CommandHistory { file: Some(file.clone()), ..Default::default() };
        if let Ok(text) = fs::read_to_string(&file) {
            for line in text.lines() {
                history.on_disk += 1;
                if let Ok(entry) = serde_json::from_str::<String>(line) {
                    history.remember(entry);
                }
            }
        }
        if history.on_disk > history.entries.len() {
            history.rewrite();
        }
        history
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn last(&self) -> Option<&str> {
        self.entries.last().map(String::as_str)
    }

    /// Add a command typed by the user. Blank commands and a repeat of the previous one are not kept.
    pub fn push(&mut self, command: &str) {
        let command = command.trim_end();
        if command.trim().is_empty() || self.last() == Some(command) {
            return;
        }
        self.remember(command.to_string());
        let Some(file) = self.file.clone() else { return };
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let line = serde_json::to_string(command).unwrap_or_default();
        if fs::OpenOptions::new().create(true).append(true).open(&file).and_then(|mut f| writeln!(f, "{line}")).is_ok() {
            self.on_disk += 1;
        }
        if self.on_disk > MAX_ENTRIES * 2 {
            self.rewrite();
        }
    }

    fn remember(&mut self, command: String) {
        if self.last() != Some(command.as_str()) {
            self.entries.push(command);
        }
        if self.entries.len() > MAX_ENTRIES {
            self.entries.drain(..self.entries.len() - MAX_ENTRIES);
        }
    }

    /// Replace the file by what is in memory, through a temporary file so a crash cannot leave half a history.
    fn rewrite(&mut self) {
        let Some(file) = self.file.clone() else { return };
        let text: String = self.entries.iter().map(|e| serde_json::to_string(e).unwrap_or_default() + "\n").collect();
        let temp = file.with_extension("tmp");
        if fs::write(&temp, text).and_then(|_| fs::rename(&temp, &file)).is_ok() {
            self.on_disk = self.entries.len();
        } else {
            let _ = fs::remove_file(&temp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn commands_survive_a_restart_including_multi_line_ones() {
        let dir = TempDir::new("history");
        let file = dir.path().join("config/history.jsonl");
        let mut history = CommandHistory::load(file.clone());
        assert!(history.entries().is_empty());
        history.push("echo one");
        history.push("echo \"quoted\"\necho second line");
        history.push("git status  ");
        let reopened = CommandHistory::load(file);
        assert_eq!(reopened.entries(), ["echo one", "echo \"quoted\"\necho second line", "git status"]);
    }

    #[test]
    fn blanks_and_immediate_repeats_are_not_kept() {
        let mut history = CommandHistory::in_memory();
        for line in ["ls", "  ", "", "ls", "pwd", "ls"] {
            history.push(line);
        }
        assert_eq!(history.entries(), ["ls", "pwd", "ls"]);
        assert_eq!(history.last(), Some("ls"));
    }

    #[test]
    fn only_the_newest_entries_are_kept_and_the_file_is_compacted() {
        let dir = TempDir::new("history-trim");
        let file = dir.path().join("history.jsonl");
        let mut history = CommandHistory::load(file.clone());
        for n in 0..MAX_ENTRIES * 2 + 5 {
            history.push(&format!("command {n}"));
        }
        assert_eq!(history.entries().len(), MAX_ENTRIES);
        assert_eq!(history.last(), Some(format!("command {}", MAX_ENTRIES * 2 + 4).as_str()));
        let lines = fs::read_to_string(&file).unwrap().lines().count();
        assert!(lines <= MAX_ENTRIES * 2, "{lines} lines on disk");
        let reopened = CommandHistory::load(file);
        assert_eq!(reopened.entries(), history.entries());
    }

    #[test]
    fn damaged_lines_are_skipped() {
        let dir = TempDir::new("history-damaged");
        let file = dir.path().join("history.jsonl");
        fs::write(&file, "\"good\"\nnot json\n\n\"also good\"\n").unwrap();
        let history = CommandHistory::load(file);
        assert_eq!(history.entries(), ["good", "also good"]);
    }

    #[test]
    fn two_sessions_can_append_to_the_same_file() {
        let dir = TempDir::new("history-shared");
        let file = dir.path().join("history.jsonl");
        let mut first = CommandHistory::load(file.clone());
        let mut second = CommandHistory::load(file.clone());
        first.push("from the first window");
        second.push("from the second window");
        let third = CommandHistory::load(file);
        assert_eq!(third.entries(), ["from the first window", "from the second window"]);
    }
}

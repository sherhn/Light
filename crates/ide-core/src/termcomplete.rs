//! Tab completion for the terminal's command line: the first word of a command from the terminal's own commands
//! and the programs on `PATH`, every other word (and any word that contains a path separator) from the files and
//! directories on disk.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// What completion may look at.
pub struct Env<'a> {
    pub cwd: &'a Path,
    /// What `~` stands for.
    pub home: Option<&'a Path>,
    /// Names offered for the first word of a command.
    pub commands: &'a [String],
    /// Written after a directory name, and tried as a separator in what was typed (`/` is always understood).
    pub separator: char,
    /// File names are compared without regard to case (Windows).
    pub ignore_case: bool,
}

/// The result of one Tab press.
#[derive(Debug, PartialEq, Eq)]
pub struct Completion {
    /// The text from `start` to the cursor is replaced by `text`.
    pub start: usize,
    pub text: String,
    /// Everything that matched, for listing when `text` does not narrow the choice any further.
    pub matches: Vec<String>,
}

/// Commands that only take directories.
const DIRECTORY_COMMANDS: [&str; 5] = ["cd", "chdir", "pushd", "rmdir", "rd"];

/// The word being typed at `cursor` (a byte index into `line`) and where it sits in the command.
struct Word {
    start: usize,
    /// The quote still open at the cursor.
    quote: Option<char>,
    /// Its text without quotes.
    text: String,
    /// Is it the first word of a command (the line start or after `;`, `|`, `&`)?
    first: bool,
    /// The command it belongs to.
    command: String,
}

fn scan(before_cursor: &str) -> Word {
    let mut quote: Option<char> = None;
    let mut start = 0;
    let mut first = true;
    let mut command = String::new();
    let mut in_word = false;
    let mut text = String::new();
    let mut words = 0;
    for (index, c) in before_cursor.char_indices() {
        if let Some(open) = quote {
            if c == open {
                quote = None;
            } else {
                text.push(c);
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                if !in_word {
                    in_word = true;
                    start = index;
                }
                quote = Some(c);
            }
            c if c.is_whitespace() || matches!(c, ';' | '|' | '&') => {
                if in_word {
                    if words == 0 {
                        command = text.to_lowercase();
                    }
                    words += 1;
                    in_word = false;
                    text.clear();
                }
                if matches!(c, ';' | '|' | '&') {
                    words = 0;
                    command.clear();
                }
                first = words == 0;
            }
            _ => {
                if !in_word {
                    in_word = true;
                    start = index;
                }
                text.push(c);
            }
        }
    }
    if !in_word {
        start = before_cursor.len();
    }
    Word { start, quote, text, first: first && words == 0, command }
}

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

fn starts_with(name: &str, prefix: &str, ignore_case: bool) -> bool {
    if ignore_case { name.to_lowercase().starts_with(&prefix.to_lowercase()) } else { name.starts_with(prefix) }
}

/// The longest start `first` shares with all `names`, in the casing of `first`.
fn common_prefix(first: &str, names: &[String], ignore_case: bool) -> String {
    let mut len = first.chars().count();
    for name in names {
        let same = first.chars().zip(name.chars()).take_while(|(a, b)| a == b || (ignore_case && a.to_lowercase().eq(b.to_lowercase()))).count();
        len = len.min(same);
    }
    first.chars().take(len).collect()
}

/// Complete the word before `cursor`. `None` when there is nothing to offer.
pub fn complete(line: &str, cursor: usize, env: &Env) -> Option<Completion> {
    let cursor = cursor.min(line.len());
    if !line.is_char_boundary(cursor) {
        return None;
    }
    let word = scan(&line[..cursor]);
    let path_like = word.text.contains(is_separator) || word.text.starts_with('~') || word.text.starts_with('.') || word.text.chars().nth(1) == Some(':');
    if word.first && !path_like {
        return complete_command(&word, env);
    }
    complete_path(&word, env)
}

fn complete_command(word: &Word, env: &Env) -> Option<Completion> {
    if word.text.is_empty() {
        return None;
    }
    let found: BTreeSet<&String> = env.commands.iter().filter(|c| starts_with(c, &word.text, env.ignore_case)).collect();
    let mut names: Vec<String> = found.into_iter().cloned().collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
    finish(word, "", &word.text, &names, false, env)
}

fn complete_path(word: &Word, env: &Env) -> Option<Completion> {
    if word.text == "~" {
        // `~` alone stands for the home directory: continue into it
        let home = env.home?;
        return Some(Completion { start: word.start, text: format!("~{}", env.separator), matches: vec![home.display().to_string()] });
    }
    let split = word.text.rfind(is_separator).map_or(0, |i| i + 1);
    let (dir_part, prefix) = word.text.split_at(split);
    // the folder as the file system reads it: a shell that writes `\` means `/` (only Windows itself reads both)
    let on_disk = if env.separator == '\\' { dir_part.replace('\\', "/") } else { dir_part.to_string() };
    let dir = if on_disk.is_empty() {
        env.cwd.to_path_buf()
    } else if let Some(rest) = on_disk.strip_prefix('~') {
        let home = env.home?;
        home.join(rest.trim_start_matches(is_separator))
    } else if Path::new(&on_disk).is_absolute() || on_disk.starts_with(is_separator) {
        PathBuf::from(&on_disk)
    } else {
        env.cwd.join(&on_disk)
    };
    let only_directories = DIRECTORY_COMMANDS.contains(&word.command.as_str());
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.starts_with('.') && !prefix.starts_with('.')) || !starts_with(&name, prefix, env.ignore_case) {
            continue;
        }
        let is_dir = entry.path().is_dir();
        if only_directories && !is_dir {
            continue;
        }
        names.push(if is_dir { format!("{name}{}", env.separator) } else { name });
    }
    names.sort_by_key(|n| n.to_lowercase());
    finish(word, dir_part, prefix, &names, true, env)
}

/// Turn the matches into the text that replaces the word: the longest common start (the whole name when there is
/// one match), quoted when it holds spaces or the word was already quoted. `typed` is the part of the word after
/// `dir_part`; it keeps the casing it was typed in unless a single name takes its place.
fn finish(word: &Word, dir_part: &str, typed: &str, names: &[String], is_path: bool, env: &Env) -> Option<Completion> {
    let first = names.first()?;
    let unique = names.len() == 1;
    let body = if unique {
        first.clone()
    } else {
        // the rest is taken from a name that agrees with the typed casing, so `READ` becomes `README`, not `READme`
        let model = names.iter().find(|n| n.starts_with(typed)).unwrap_or(first);
        let shared = common_prefix(model, names, env.ignore_case);
        let skip = typed.chars().count();
        format!("{typed}{}", shared.chars().skip(skip).collect::<String>())
    };
    let full = format!("{dir_part}{body}");
    let ends_with_directory = is_path && unique && full.ends_with(env.separator);
    let quote = word.quote.or_else(|| full.contains(char::is_whitespace).then_some('"'));
    let mut text = match quote {
        Some(q) => format!("{q}{full}"),
        None => full,
    };
    if unique && !ends_with_directory {
        if let Some(q) = quote {
            text.push(q);
        }
        text.push(' ');
    }
    Some(Completion { start: word.start, text, matches: names.to_vec() })
}

/// Names of the programs found on `PATH`, for the first word of a command.
pub fn path_commands() -> Vec<String> {
    let Some(path) = std::env::var_os("PATH") else { return vec![] };
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()).split(';').filter(|e| !e.is_empty()).map(|e| e.to_lowercase()).collect()
    } else {
        vec![]
    };
    let mut found = BTreeSet::new();
    for dir in std::env::split_paths(&path) {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if cfg!(windows) {
                let lower = name.to_lowercase();
                if let Some(ext) = extensions.iter().find(|e| lower.ends_with(e.as_str())) {
                    found.insert(name[..name.len() - ext.len()].to_string());
                }
            } else if is_executable(&entry.path()) {
                found.insert(name);
            }
            if found.len() > 50_000 {
                return found.into_iter().collect();
            }
        }
    }
    found.into_iter().collect()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn tree() -> TempDir {
        let d = TempDir::new("complete");
        for dir in ["src", "scripts", "docs/guide", ".hidden"] {
            fs::create_dir_all(d.path().join(dir)).unwrap();
        }
        for file in ["src/main.lisp", "src/macros.lisp", "README.md", "readme-ru.md", "My Notes.txt", "scripts/build.sh", ".gitignore"] {
            fs::write(d.path().join(file), "").unwrap();
        }
        d
    }

    fn env<'a>(cwd: &'a Path, commands: &'a [String]) -> Env<'a> {
        Env { cwd, home: None, commands, separator: '/', ignore_case: true }
    }

    fn run(line: &str, env: &Env) -> Option<Completion> {
        complete(line, line.len(), env)
    }

    #[test]
    fn a_single_match_is_completed_in_full_and_directories_get_a_separator() {
        let d = tree();
        let e = env(d.path(), &[]);
        let c = run("cat do", &e).unwrap();
        assert_eq!((c.start, c.text.as_str()), (4, "docs/"));
        let c = run("cat docs/g", &e).unwrap();
        assert_eq!((c.start, c.text.as_str()), (4, "docs/guide/"));
        let c = run("cat scripts/bu", &e).unwrap();
        assert_eq!(c.text, "scripts/build.sh ", "a finished file name is followed by a space");
    }

    #[test]
    fn several_matches_complete_to_what_they_share_and_are_listed() {
        let d = tree();
        let e = env(d.path(), &[]);
        let c = run("cat src/ma", &e).unwrap();
        assert_eq!(c.text, "src/ma", "main.lisp and macros.lisp only share what is typed");
        assert_eq!(c.matches, ["macros.lisp", "main.lisp"]);
        let c = run("cat s", &e).unwrap();
        assert_eq!(c.text, "s");
        assert_eq!(c.matches, ["scripts/", "src/"]);
        let c = run("cat read", &e).unwrap();
        assert_eq!(c.text, "readme", "file names compare without case here, and what was typed keeps its casing");
        let c = run("cat READ", &e).unwrap();
        assert_eq!(c.text, "README");
        assert_eq!(c.matches.len(), 2);
    }

    #[test]
    fn hidden_files_are_offered_only_after_a_dot() {
        let d = tree();
        let e = env(d.path(), &[]);
        assert!(!run("cat ", &e).unwrap().matches.iter().any(|m| m.starts_with('.')));
        assert_eq!(run("cat .g", &e).unwrap().text, ".gitignore ");
        assert_eq!(run("cat .", &e).unwrap().matches, [".gitignore", ".hidden/"]);
    }

    #[test]
    fn cd_and_friends_only_offer_directories() {
        let d = tree();
        let e = env(d.path(), &[]);
        assert_eq!(run("cd s", &e).unwrap().matches, ["scripts/", "src/"]);
        assert_eq!(run("cd ", &e).unwrap().matches, ["docs/", "scripts/", "src/"]);
        assert_eq!(run("cat s", &e).unwrap().matches, ["scripts/", "src/"]);
        assert!(run("cd READ", &e).is_none());
        assert!(run("cat READ", &e).is_some());
    }

    #[test]
    fn names_with_spaces_are_quoted() {
        let d = tree();
        let e = env(d.path(), &[]);
        assert_eq!(run("cat My", &e).unwrap().text, "\"My Notes.txt\" ");
        assert_eq!(run("cat \"My", &e).unwrap().text, "\"My Notes.txt\" ");
        assert_eq!(run("cat 'My N", &e).unwrap().text, "'My Notes.txt' ");
    }

    #[test]
    fn the_first_word_comes_from_the_commands() {
        let d = tree();
        let commands: Vec<String> = ["cargo", "cat", "cd", "clear", "git"].map(String::from).to_vec();
        let e = env(d.path(), &commands);
        let c = run("ca", &e).unwrap();
        assert_eq!((c.start, c.text.as_str()), (0, "ca"));
        assert_eq!(c.matches, ["cargo", "cat"]);
        assert_eq!(run("gi", &e).unwrap().text, "git ");
        assert!(run("", &e).is_none(), "an empty first word would list every program");
        assert!(run("zzz", &e).is_none());
        // after a separator a new command starts
        let c = run("git status && ca", &e).unwrap();
        assert_eq!((c.start, c.matches.len()), (14, 2));
        // a word with a path separator in command position is a path
        assert_eq!(run("./sc", &e).unwrap().text, "./scripts/");
        assert_eq!(run("scripts/bu", &e).unwrap().text, "scripts/build.sh ");
    }

    #[test]
    fn the_word_at_the_cursor_is_completed_and_the_rest_is_left_alone() {
        let d = tree();
        let e = env(d.path(), &[]);
        let line = "cat do --flag";
        let c = complete(line, 6, &e).unwrap();
        assert_eq!((c.start, c.text.as_str()), (4, "docs/"));
        assert!(complete("cat \u{e9}", 5, &e).is_none(), "not on a character boundary");
    }

    #[test]
    fn home_and_parent_directories_and_missing_directories() {
        let d = tree();
        let home = d.path().join("docs");
        let e = Env { home: Some(&home), ..env(d.path(), &[]) };
        assert_eq!(run("cat ~/g", &e).unwrap().text, "~/guide/");
        assert_eq!(run("cat ~", &e).unwrap().text, "~/");
        let nested = d.path().join("src");
        let e = env(&nested, &[]);
        assert_eq!(run("cat ../READ", &e).unwrap().text, "../README");
        assert!(run("cat nowhere/x", &e).is_none());
    }

    #[test]
    fn backslashes_work_as_separators_for_the_windows_shell() {
        let d = tree();
        let e = Env { separator: '\\', ..env(d.path(), &[]) };
        assert_eq!(run("cd docs\\g", &e).unwrap().text, "docs\\guide\\");
        assert_eq!(run("cd d", &e).unwrap().text, "docs\\");
    }

    #[test]
    fn path_commands_lists_programs_of_the_path() {
        let found = path_commands();
        assert!(found.windows(2).all(|w| w[0] <= w[1]), "sorted");
        let probe = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(found.iter().any(|c| c.eq_ignore_ascii_case(probe)), "{probe} should be on the PATH");
    }
}

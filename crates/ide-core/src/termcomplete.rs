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

/// A complete replacement, including a partially typed suffix after the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathEdit { pub start: usize, pub end: usize, pub text: String, pub caret: usize }

pub fn path_word(line: &str, cursor: usize) -> bool {
    if cursor > line.len() || !line.is_char_boundary(cursor) { return false }
    let word=scan(&line[..cursor]);
    !word.first || word.text.contains(is_separator) || word.text.starts_with(['~','.']) || word.text.chars().nth(1)==Some(':')
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
    let (dir_part,prefix,names)=path_names(&word.text,env,None,DIRECTORY_COMMANDS.contains(&word.command.as_str()))?;
    finish(word, &dir_part, &prefix, &names, true, env)
}

fn path_names(text: &str, env: &Env, root: Option<&Path>, only_directories: bool) -> Option<(String,String,Vec<String>)> {
    let split = text.rfind(is_separator).map_or(0, |i| i + 1);
    let (dir_part, prefix) = text.split_at(split);
    // the folder as the file system reads it: a shell that writes `\` means `/` (only Windows itself reads both)
    let on_disk = if env.separator == '\\' { dir_part.replace('\\', "/") } else { dir_part.to_string() };
    let dir = if on_disk.is_empty() {
        env.cwd.to_path_buf()
    } else if let Some(rest) = on_disk.strip_prefix('~') {
        let home = env.home?;
        home.join(rest.trim_start_matches(is_separator))
    } else if on_disk.starts_with('/') && root.is_some() {
        root?.join(on_disk.trim_start_matches('/'))
    } else if Path::new(&on_disk).is_absolute() || on_disk.starts_with(is_separator) {
        PathBuf::from(&on_disk)
    } else {
        env.cwd.join(&on_disk)
    };
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
    names.sort_by_cached_key(|n| (n.to_lowercase(),n.clone()));
    Some((dir_part.into(),prefix.into(),names))
}

/// Complete a literal path field. Spaces are literal; shell quoting is never inserted.
/// `root` maps absolute Linux paths to a WSL UNC root when running on Windows.
pub fn path_field(text: &str, cursor: usize, env: &Env, root: Option<&Path>, directories_only: bool) -> Option<PathEdit> {
    if cursor > text.len() || !text.is_char_boundary(cursor) || cursor==0 { return None }
    let before=&text[..cursor];
    let replacement=if before=="~" && env.home.is_some() {format!("~{}",env.separator)}
    else if cfg!(windows) && before.len()==2 && before.ends_with(':') && before.as_bytes()[0].is_ascii_alphabetic() {
        let drive=format!("{before}\\");if !Path::new(&drive).is_dir() {return None} drive
    } else {
        let (directory,_,names)=path_names(before,env,root,directories_only)?;
        format!("{directory}{}",names.first()?)
    };
    let mut end=text[cursor..].find(is_separator).map_or(text.len(),|index|cursor+index);
    if replacement.ends_with(is_separator) && end<text.len() {end+=1;}
    let caret=replacement.len();
    Some(PathEdit {start:0,end,text:replacement,caret})
}

/// Complete an argument/path in a command line with the first matching entry.
/// Command-name completion keeps its existing common-prefix/list behavior in `complete`.
pub fn first_path(line: &str, cursor: usize, env: &Env, root: Option<&Path>) -> Option<PathEdit> {
    if !path_word(line,cursor) {return None}
    let word=scan(&line[..cursor]);
    let mut full=if word.text=="~" && env.home.is_some() {format!("~{}",env.separator)}
    else if cfg!(windows) && word.text.len()==2 && word.text.ends_with(':') && word.text.as_bytes()[0].is_ascii_alphabetic() {
        let drive=format!("{}\\",word.text);if !Path::new(&drive).is_dir() {return None} drive
    } else {
        let (directory,_,names)=path_names(&word.text,env,root,DIRECTORY_COMMANDS.contains(&word.command.as_str()))?;
        format!("{directory}{}",names.first()?)
    };
    // An absolute spelling works both in cmd.exe (which does not expand ~) and
    // inside shell quotes (where POSIX shells deliberately do not expand it).
    if let Some(rest)=full.strip_prefix("~/").or_else(||full.strip_prefix("~\\")) {
        let home=env.home?;
        let displayed=if let Some(root)=root {format!("/{}",home.strip_prefix(root).ok()?.to_string_lossy().replace('\\',"/"))}
            else {home.to_string_lossy().into_owned()};
        full=format!("{}{sep}{rest}",displayed.trim_end_matches(is_separator),sep=env.separator);
    }
    let directory=full.ends_with(is_separator);
    let windows=env.separator=='\\';
    let dangerous=full.contains(['&','|',';','<','>','(',')','$','`','!','\'','"']);
    let quoted=word.quote.is_some()||full.contains(char::is_whitespace)||dangerous;
    let quote=if !windows && dangerous {'\''}else{word.quote.unwrap_or('"')};
    let mut text=if quoted {
        let escaped=if windows {full} else if quote=='\'' {full.replace('\'',"'\\''")} else {full.replace('\\',"\\\\").replace('"',"\\\"").replace('$',"\\$").replace('`',"\\`")};
        format!("{quote}{escaped}{quote}")
    } else {full};
    let mut end=cursor;let mut open=word.quote;
    for (index,ch) in line[cursor..].char_indices() {
        if let Some(quote)=open {if ch==quote {open=None;}}
        else if ch.is_whitespace()||matches!(ch,';'|'|'|'&') {break;}
        else if matches!(ch,'\''|'"') {open=Some(ch);}
        end=cursor+index+ch.len_utf8();
    }
    let mut caret=word.start+text.len();
    if directory && quoted {caret-=1;}
    else if !directory {
        if let Some(ch)=line[end..].chars().next().filter(|c|c.is_whitespace()) {caret+=ch.len_utf8();}
        else {text.push(' ');caret+=1;}
    }
    Some(PathEdit {start:word.start,end,text,caret})
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

    fn apply(line:&str,edit:PathEdit)->(String,usize) {
        let mut line=line.to_string();line.replace_range(edit.start..edit.end,&edit.text);(line,edit.caret)
    }
    #[test]
    fn first_path_chooses_a_complete_name_when_several_entries_match() {
        let d=tree();let env=env(d.path(),&[]);
        let line="cd s";let (line,caret)=apply(line,first_path(line,line.len(),&env,None).unwrap());
        assert_eq!(line,"cd scripts/");assert_eq!(caret,line.len());
        let line="cat src/ma";assert_eq!(apply(line,first_path(line,line.len(),&env,None).unwrap()).0,"cat src/macros.lisp ");
        assert!(!path_word("ca",2));assert!(first_path("ca",2,&env,None).is_none());
    }
    #[test]
    fn literal_fields_keep_spaces_unicode_and_the_suffix_after_the_completed_component() {
        let d=tree();fs::create_dir(d.path().join("Мои проекты")).unwrap();let env=env(d.path(),&[]);
        let path="Мои пр";let (path,_)=apply(path,path_field(path,path.len(),&env,None,true).unwrap());
        assert_eq!(path,"Мои проекты/");
        let path="src/maTYPO";assert_eq!(apply(path,path_field(path,6,&env,None,false).unwrap()).0,"src/macros.lisp");
        let path="docs/guTYPO/more";assert_eq!(apply(path,path_field(path,7,&env,None,true).unwrap()).0,"docs/guide/more");
        assert!(path_field("src/ma",6,&env,None,true).is_none());
    }
    #[test]
    fn first_path_replaces_partial_suffixes_and_closes_quotes_without_losing_arguments() {
        let d=tree();let env=env(d.path(),&[]);
        let line="cat \"My NoTYPO.txt\" --flag";let cursor=line.find("TYPO").unwrap();
        let (line,caret)=apply(line,first_path(line,cursor,&env,None).unwrap());
        assert_eq!(line,"cat \"My Notes.txt\" --flag");assert_eq!(&line[caret..],"--flag");
        fs::create_dir(d.path().join("My Folder")).unwrap();
        let line="cd My F"; // A path containing spaces must be quoted in a command.
        assert!(first_path(line,line.len(),&env,None).is_none());
        let line="cd \"My F";let (line,caret)=apply(line,first_path(line,line.len(),&env,None).unwrap());
        assert_eq!(line,"cd \"My Folder/\"");assert_eq!(&line[caret..],"\"");
    }
    #[test]
    fn linux_absolute_paths_can_be_mapped_to_a_virtual_wsl_root() {
        let d=tree();let root=d.path().join("linux");fs::create_dir_all(root.join("home/user/projects")).unwrap();
        let env=env(d.path(),&[]);
        let path="/home/us";assert_eq!(apply(path,path_field(path,path.len(),&env,Some(&root),true).unwrap()).0,"/home/user/");
        let line="cd /home/us";assert_eq!(apply(line,first_path(line,line.len(),&env,Some(&root)).unwrap()).0,"cd /home/user/");
        fs::create_dir(root.join("home/user/My Projects")).unwrap();
        let home=root.join("home/user");let env=Env {home:Some(&home),..env};
        let line="cat ~/My";assert_eq!(apply(line,first_path(line,line.len(),&env,Some(&root)).unwrap()).0,"cat \"/home/user/My Projects/\"");
    }
    #[test]
    fn posix_shell_metacharacters_in_file_names_are_quoted_as_data() {
        let d=tree();fs::write(d.path().join("data$(whoami)&test.txt"),"").unwrap();let env=env(d.path(),&[]);
        let line="cat data";assert_eq!(apply(line,first_path(line,line.len(),&env,None).unwrap()).0,"cat 'data$(whoami)&test.txt' ");
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

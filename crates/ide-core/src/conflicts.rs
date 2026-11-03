//! The conflicts Git leaves in a file it could not merge: the blocks between `<<<<<<<` and `>>>>>>>` with both
//! versions (and, in the `diff3` style, the common base between `|||||||` and `=======`), and the text with one of them
//! chosen.

/// One conflict of a file. Lines are 0-based; `start` is the line of `<<<<<<<`, `end` the line of `>>>>>>>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub start: usize,
    /// The line of `|||||||` when the base is shown.
    pub base_marker: Option<usize>,
    /// The line of `=======`.
    pub middle: usize,
    pub end: usize,
    /// What comes after the markers: the branch (or commit) of each side.
    pub ours_label: String,
    pub theirs_label: String,
    /// The lines of each side, each with its line ending.
    pub ours: String,
    pub base: Option<String>,
    pub theirs: String,
}

/// How to resolve a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The version of the current branch (the upper one).
    Ours,
    /// The version being merged in (the lower one).
    Theirs,
    /// Both, ours first.
    Both,
}

fn marker(line: &str, sign: char) -> Option<&str> {
    let body = line.trim_end_matches(['\r', '\n']);
    let rest = body.strip_prefix(&sign.to_string().repeat(7))?;
    // exactly seven signs, then the end or a space with the label
    (rest.is_empty() || rest.starts_with(' ')).then(|| rest.trim())
}

/// The conflicts of `text`, in order. A block that is not closed is not a conflict.
pub fn find(text: &str) -> Vec<Conflict> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(ours_label) = marker(lines[i], '<') else {
            i += 1;
            continue;
        };
        let start = i;
        let (mut base_marker, mut middle, mut end) = (None, None, None);
        let mut j = i + 1;
        while j < lines.len() {
            if marker(lines[j], '<').is_some() {
                break;
            }
            if middle.is_none() && base_marker.is_none() && marker(lines[j], '|').is_some() {
                base_marker = Some(j);
            } else if middle.is_none() && marker(lines[j], '=').is_some_and(str::is_empty) {
                middle = Some(j);
            } else if middle.is_some() && let Some(label) = marker(lines[j], '>') {
                end = Some((j, label.to_string()));
                break;
            }
            j += 1;
        }
        match (middle, end) {
            (Some(middle), Some((end, theirs_label))) => {
                let join = |a: usize, b: usize| lines[a..b].concat();
                let ours_end = base_marker.unwrap_or(middle);
                out.push(Conflict {
                    start,
                    base_marker,
                    middle,
                    end,
                    ours_label: ours_label.to_string(),
                    theirs_label,
                    ours: join(start + 1, ours_end),
                    base: base_marker.map(|b| join(b + 1, middle)),
                    theirs: join(middle + 1, end),
                });
                i = end + 1;
            }
            _ => i = j.max(i + 1),
        }
    }
    out
}

/// `text` with conflict number `index` (of [`find`]) replaced by what `choice` keeps.
pub fn resolve(text: &str, index: usize, choice: Choice) -> Option<String> {
    let c = find(text).into_iter().nth(index)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let kept = match choice {
        Choice::Ours => c.ours.clone(),
        Choice::Theirs => c.theirs.clone(),
        Choice::Both => {
            let mut s = c.ours.clone();
            if !s.is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            s + &c.theirs
        }
    };
    let mut out = lines[..c.start].concat();
    out.push_str(&kept);
    // the line of `>>>>>>>` was the last of the file without a line break: so is what replaces it
    out.push_str(&lines[c.end + 1..].concat());
    if !lines[c.end].ends_with('\n') && out.ends_with('\n') && c.end + 1 == lines.len() {
        out.pop();
        if out.ends_with('\r') {
            out.pop();
        }
    }
    Some(out)
}

/// Resolve every conflict of `text` the same way.
pub fn resolve_all(text: &str, choice: Choice) -> String {
    let mut t = text.to_string();
    while let Some(next) = resolve(&t, 0, choice) {
        t = next;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERGE: &str = "fn main() {\n<<<<<<< HEAD\n    println!(\"ours\");\n=======\n    println!(\"theirs\");\n    println!(\"two\");\n>>>>>>> feature\n}\n<<<<<<< HEAD\nlast ours\n||||||| merged common ancestors\nlast base\n=======\nlast theirs\n>>>>>>> feature";

    #[test]
    fn conflicts_are_found_with_both_sides_and_the_base() {
        let c = find(MERGE);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].start, c[0].middle, c[0].end, c[0].base_marker), (1, 3, 6, None));
        assert_eq!((c[0].ours_label.as_str(), c[0].theirs_label.as_str()), ("HEAD", "feature"));
        assert_eq!(c[0].ours, "    println!(\"ours\");\n");
        assert_eq!(c[0].theirs, "    println!(\"theirs\");\n    println!(\"two\");\n");
        assert_eq!((c[1].base_marker, c[1].base.as_deref(), c[1].ours.as_str(), c[1].theirs.as_str()), (Some(10), Some("last base\n"), "last ours\n", "last theirs\n"));
        // text that only looks like markers is not a conflict
        assert!(find("<<<<<<< x\nno middle\n>>>>>>> y\n").is_empty());
        assert!(find("<<<<<<<< eight\n=======\n>>>>>>> y\n").is_empty());
        assert!(find("a\n=======\nb\n").is_empty());
    }

    #[test]
    fn a_choice_replaces_the_block() {
        assert_eq!(resolve(MERGE, 0, Choice::Ours).unwrap(), MERGE.replace("<<<<<<< HEAD\n    println!(\"ours\");\n=======\n    println!(\"theirs\");\n    println!(\"two\");\n>>>>>>> feature\n", "    println!(\"ours\");\n"));
        let theirs = resolve(MERGE, 1, Choice::Theirs).unwrap();
        assert!(theirs.ends_with("}\nlast theirs"), "no line break added at the end: {theirs:?}");
        let both = resolve_all(MERGE, Choice::Both);
        assert_eq!(both, "fn main() {\n    println!(\"ours\");\n    println!(\"theirs\");\n    println!(\"two\");\n}\nlast ours\nlast theirs");
        assert!(find(&both).is_empty());
        assert_eq!(resolve(MERGE, 2, Choice::Ours), None);
        let crlf = "a\r\n<<<<<<< HEAD\r\nx\r\n=======\r\ny\r\n>>>>>>> b\r\nz\r\n";
        assert_eq!(resolve(crlf, 0, Choice::Theirs).unwrap(), "a\r\ny\r\nz\r\n");
    }
}

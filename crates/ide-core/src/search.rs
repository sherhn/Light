//! Fuzzy matching (Search Everywhere, Go to File, Find Action) and text search.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    /// Char indices (not bytes) of the matched characters in the candidate.
    pub positions: Vec<usize>,
}

fn is_boundary(prev: Option<char>, cur: char) -> bool {
    match prev {
        None => true,
        Some(p) => matches!(p, '-' | '_' | '/' | '\\' | '.' | ' ' | ':' | '*' | '+') || (p.is_lowercase() && cur.is_uppercase()),
    }
}

/// Subsequence fuzzy match with word-boundary and adjacency bonuses (best alignment via DP).
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(FuzzyMatch { score: 0, positions: vec![] });
    }
    // cheap rejection before the DP: the query must be a case-insensitive subsequence of the candidate
    {
        let mut qi = q.iter().flat_map(|ch| ch.to_lowercase()).peekable();
        for ch in candidate.chars() {
            if let Some(&want) = qi.peek()
                && ch.to_lowercase().next() == Some(want)
            {
                qi.next();
            }
        }
        if qi.peek().is_some() {
            return None;
        }
    }
    let c: Vec<char> = candidate.chars().collect();
    if q.len() > c.len() {
        return None;
    }
    let ql: Vec<char> = q.iter().flat_map(|ch| ch.to_lowercase()).collect();
    let cl: Vec<char> = c.iter().map(|ch| ch.to_lowercase().next().unwrap_or(*ch)).collect();
    if ql.len() != q.len() {
        return None;
    }
    let n = c.len();
    const NEG: i32 = i32::MIN / 2;
    // best[i][j]: best score with q[i] matched at c[j]
    let mut best = vec![vec![NEG; n]; q.len()];
    let mut from = vec![vec![usize::MAX; n]; q.len()];
    for j in 0..n {
        if cl[j] != ql[0] {
            continue;
        }
        let mut s = 10;
        if is_boundary(j.checked_sub(1).map(|k| c[k]), c[j]) {
            s += 12;
        }
        if c[j] == q[0] {
            s += 2;
        }
        s -= (j as i32).min(10); // prefer early matches
        best[0][j] = s;
    }
    for i in 1..q.len() {
        for j in i..n {
            if cl[j] != ql[i] {
                continue;
            }
            let mut bonus = 10;
            if is_boundary(Some(c[j - 1]), c[j]) {
                bonus += 12;
            }
            if c[j] == q[i] {
                bonus += 2;
            }
            for k in (i - 1)..j {
                if best[i - 1][k] == NEG {
                    continue;
                }
                let gap = (j - k - 1) as i32;
                let adj = if gap == 0 { 15 } else { -gap.min(6) };
                let s = best[i - 1][k] + bonus + adj;
                if s > best[i][j] {
                    best[i][j] = s;
                    from[i][j] = k;
                }
            }
        }
    }
    let last = q.len() - 1;
    let (mut j, mut score) = (usize::MAX, NEG);
    for (jj, &s) in best[last].iter().enumerate() {
        if s > score {
            score = s;
            j = jj;
        }
    }
    if score == NEG {
        return None;
    }
    let mut positions = vec![0; q.len()];
    let mut i = last;
    loop {
        positions[i] = j;
        if i == 0 {
            break;
        }
        j = from[i][j];
        i -= 1;
    }
    // shorter candidates rank higher; exact (case-insensitive) matches get a large bonus
    score -= (n as i32 - q.len() as i32).min(30) / 2;
    if cl == ql {
        score += 40;
    }
    Some(FuzzyMatch { score, positions })
}

/// Rank `items` against `query`, best first. `key` extracts the text to match.
pub fn rank<T, F: Fn(&T) -> String>(query: &str, items: &[T], key: F, limit: usize) -> Vec<(usize, FuzzyMatch)> {
    let mut out: Vec<(usize, FuzzyMatch)> = items.iter().enumerate().filter_map(|(i, it)| fuzzy_match(query, &key(it)).map(|m| (i, m))).collect();
    out.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    out.truncate(limit);
    out
}

/// Fuzzy match a path: prefers matches within the file name.
pub fn match_path(query: &str, path: &str) -> Option<FuzzyMatch> {
    let name_start = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let name = &path[name_start..];
    if let Some(mut m) = fuzzy_match(query, name) {
        m.score += 30;
        let off = path[..name_start].chars().count();
        m.positions.iter_mut().for_each(|p| *p += off);
        return Some(m);
    }
    fuzzy_match(query, path)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TextQuery {
    pub case_sensitive: bool,
    pub whole_word: bool,
}

fn is_symbol_char(c: char) -> bool {
    !(c.is_whitespace() || "()[]{}\"';`,".contains(c))
}

/// Byte ranges of all matches of `needle` in `text`.
pub fn find_in_text(text: &str, needle: &str, opts: TextQuery) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return vec![];
    }
    let (hay, pat): (std::borrow::Cow<str>, std::borrow::Cow<str>) = if opts.case_sensitive {
        (text.into(), needle.into())
    } else {
        (text.to_lowercase().into(), needle.to_lowercase().into())
    };
    // Lowercasing can change byte lengths for exotic characters; fall back to ASCII-only folding then.
    let (hay, pat) = if hay.len() != text.len() {
        (text.to_ascii_lowercase().into(), needle.to_ascii_lowercase().into())
    } else {
        (hay, pat)
    };
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = hay[from..].find(pat.as_ref()) {
        let s = from + i;
        let e = s + pat.len();
        let ok = !opts.whole_word || {
            let before = text[..s].chars().next_back();
            let after = text[e..].chars().next();
            !before.is_some_and(is_symbol_char) && !after.is_some_and(is_symbol_char)
        };
        if ok && text.is_char_boundary(s) && text.is_char_boundary(e) {
            out.push((s, e));
        }
        from = e.max(s + 1);
        while from < hay.len() && !hay.is_char_boundary(from) {
            from += 1;
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHit {
    pub path: PathBuf,
    /// 0-based line.
    pub line: usize,
    /// Char column of the match.
    pub col: usize,
    pub len: usize,
    pub line_text: String,
}

const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024;

pub fn is_probably_text(bytes: &[u8]) -> bool {
    !bytes.iter().take(8192).any(|&b| b == 0)
}

/// Walk project files honouring .gitignore, skipping VCS metadata and build output.
pub fn walk_files(root: &Path) -> impl Iterator<Item = PathBuf> {
    walk_project(root, &crate::workspace::Hidden::default())
}

/// [`walk_files`] without the folders and files the project hides.
pub fn walk_project(root: &Path, hidden: &crate::workspace::Hidden) -> impl Iterator<Item = PathBuf> + use<> {
    let root = root.to_path_buf();
    let hidden = hidden.clone();
    WalkBuilder::new(&root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(move |e| {
            let name = e.file_name().to_string_lossy();
            !matches!(name.as_ref(), ".git" | ".idea" | "node_modules" | "target" | "fasl-cache" | ".light" | ".DS_Store")
                && !hidden.is_hidden(e.path(), e.file_type().is_some_and(|t| t.is_dir()))
        })
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
}

pub fn find_in_files(root: &Path, needle: &str, opts: TextQuery, limit: usize) -> Vec<FileHit> {
    find_in_project(root, &crate::workspace::Hidden::default(), needle, opts, limit)
}

/// [`find_in_files`] in the files the project does not hide.
pub fn find_in_project(root: &Path, hidden: &crate::workspace::Hidden, needle: &str, opts: TextQuery, limit: usize) -> Vec<FileHit> {
    let mut hits = Vec::new();
    if needle.is_empty() {
        return hits;
    }
    for path in walk_project(root, hidden) {
        if hits.len() >= limit {
            break;
        }
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.len() > MAX_FILE_SIZE {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if !is_probably_text(&bytes) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else { continue };
        for (line_no, line) in text.lines().enumerate() {
            for (s, e) in find_in_text(line, needle, opts) {
                hits.push(FileHit {
                    path: path.clone(),
                    line: line_no,
                    col: line[..s].chars().count(),
                    len: line[s..e].chars().count(),
                    line_text: line.trim_end().to_string(),
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matching() {
        assert!(fuzzy_match("gtf", "go-to-file").is_some());
        assert!(fuzzy_match("xyz", "go-to-file").is_none());
        assert!(fuzzy_match("", "anything").is_some());
        assert!(fuzzy_match("toolong", "tl").is_none());
    }

    #[test]
    fn boundary_matches_beat_scattered() {
        let a = fuzzy_match("gtf", "go-to-file").unwrap();
        let b = fuzzy_match("gtf", "gaxtxxfxx").unwrap();
        assert!(a.score > b.score);
        assert_eq!(a.positions, vec![0, 3, 6]);
    }

    #[test]
    fn exact_and_prefix_rank_first() {
        let items = ["main.lisp", "domain-model.lisp", "main", "remaining.lisp"];
        let r = rank("main", &items, |s| s.to_string(), 10);
        assert_eq!(items[r[0].0], "main");
        assert_eq!(items[r[1].0], "main.lisp");
        assert!(r.iter().any(|(i, _)| items[*i] == "remaining.lisp"));
    }

    #[test]
    fn camel_case_boundaries() {
        let m = fuzzy_match("gl", "GoLine").unwrap();
        assert_eq!(m.positions, vec![0, 2]);
    }

    #[test]
    fn path_matching_prefers_file_name() {
        let a = match_path("util", "src/util.lisp").unwrap();
        let b = match_path("util", "utilities/readme.md").unwrap();
        assert!(a.score > b.score);
        assert_eq!(a.positions, vec![4, 5, 6, 7]);
    }

    #[test]
    fn unicode_candidates() {
        let m = fuzzy_match("пр", "Привет").unwrap();
        assert_eq!(m.positions, vec![0, 1]);
    }

    #[test]
    fn find_text_options() {
        let t = "Foo foo (foo-bar) foo";
        assert_eq!(find_in_text(t, "foo", TextQuery::default()).len(), 4);
        assert_eq!(find_in_text(t, "foo", TextQuery { case_sensitive: true, whole_word: false }).len(), 3);
        // whole word: `foo-bar` is one lisp symbol
        let w = find_in_text(t, "foo", TextQuery { case_sensitive: false, whole_word: true });
        assert_eq!(w, vec![(0, 3), (4, 7), (18, 21)]);
        assert!(find_in_text(t, "", TextQuery::default()).is_empty());
    }

    #[test]
    fn find_text_unicode_safe() {
        let t = "ÄÖ äö İi";
        let r = find_in_text(t, "ä", TextQuery::default());
        assert!(!r.is_empty());
        for (s, e) in r {
            assert!(t.is_char_boundary(s) && t.is_char_boundary(e));
        }
    }

    #[test]
    fn find_in_files_walks_and_skips_binary_and_ignored() {
        let dir = crate::testutil::TempDir::new("find");
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("src/a.lisp"), "(defun needle ())\n(needle)\n").unwrap();
        std::fs::write(dir.path().join("target/b.lisp"), "needle").unwrap();
        std::fs::write(dir.path().join("bin.dat"), b"needle\0\0").unwrap();
        let hits = find_in_files(dir.path(), "needle", TextQuery::default(), 100);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.path.ends_with("a.lisp")));
        assert_eq!((hits[0].line, hits[0].col, hits[0].len), (0, 7, 6));
        assert_eq!(hits[1].line, 1);
        assert_eq!(find_in_files(dir.path(), "needle", TextQuery::default(), 1).len(), 1);
    }
}

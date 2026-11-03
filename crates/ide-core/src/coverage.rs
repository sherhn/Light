//! Code coverage: which lines a run of the tests (or of a program) executed and how often, read from the reports the
//! tools write - LCOV (`lcov.info` of Node.js, Jest, c8, coverage.py) and the JSON of `gcov --json-format`.

use std::collections::BTreeMap;

/// The lines of one file: 1-based line -> how many times it ran (0: it did not).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileCoverage {
    /// The path as the report gives it (relative to where the run was, or absolute).
    pub path: String,
    pub lines: BTreeMap<usize, u64>,
}

impl FileCoverage {
    /// (lines that ran, lines that could have).
    pub fn counts(&self) -> (usize, usize) {
        (self.lines.values().filter(|c| **c > 0).count(), self.lines.len())
    }

    /// The share of lines that ran, 0.0 to 1.0 (1.0 for a file without lines that could run).
    pub fn ratio(&self) -> f32 {
        let (hit, all) = self.counts();
        if all == 0 { 1.0 } else { hit as f32 / all as f32 }
    }
}

/// The report formats.
pub const FORMATS: &[&str] = &["lcov", "gcov-json"];

/// Read a report of `format` (one of [`FORMATS`]). The files come in the order of the report; a file named twice has
/// its counts added up.
pub fn parse(format: &str, text: &str) -> Result<Vec<FileCoverage>, String> {
    let files = match format {
        "lcov" => parse_lcov(text),
        "gcov-json" => parse_gcov_json(text)?,
        other => return Err(format!("unknown coverage format {other:?}")),
    };
    let mut merged: Vec<FileCoverage> = Vec::new();
    for f in files {
        match merged.iter_mut().find(|m| m.path == f.path) {
            Some(m) => {
                for (line, count) in f.lines {
                    *m.lines.entry(line).or_default() += count;
                }
            }
            None => merged.push(f),
        }
    }
    Ok(merged)
}

fn parse_lcov(text: &str) -> Vec<FileCoverage> {
    let mut out = Vec::new();
    let mut current: Option<FileCoverage> = None;
    for line in text.lines().map(str::trim) {
        if let Some(path) = line.strip_prefix("SF:") {
            current = Some(FileCoverage { path: path.to_string(), lines: BTreeMap::new() });
        } else if let Some(da) = line.strip_prefix("DA:") {
            let mut parts = da.split(',');
            if let (Some(c), Some(Ok(n)), Some(Ok(count))) = (current.as_mut(), parts.next().map(str::parse::<usize>), parts.next().map(|s| s.parse::<u64>())) {
                *c.lines.entry(n).or_default() += count;
            }
        } else if line == "end_of_record"
            && let Some(c) = current.take()
        {
            out.push(c);
        }
    }
    out.extend(current);
    out
}

/// `gcov --json-format --stdout`: one JSON document per object file, each line of the output (other text, such as what
/// the program printed before, is not JSON and is skipped).
fn parse_gcov_json(text: &str) -> Result<Vec<FileCoverage>, String> {
    let mut out = Vec::new();
    let mut any = false;
    for doc in text.lines().map(str::trim).filter(|l| l.starts_with('{')) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(doc) else { continue };
        any = true;
        let cwd = value["current_working_directory"].as_str().unwrap_or_default();
        for f in value["files"].as_array().into_iter().flatten() {
            let name = f["file"].as_str().unwrap_or_default();
            let path = if name.starts_with('/') || cwd.is_empty() { name.to_string() } else { format!("{}/{name}", cwd.trim_end_matches('/')) };
            let mut lines = BTreeMap::new();
            for l in f["lines"].as_array().into_iter().flatten() {
                if let (Some(n), Some(count)) = (l["line_number"].as_u64(), l["count"].as_u64()) {
                    *lines.entry(n as usize).or_default() += count;
                }
            }
            out.push(FileCoverage { path, lines });
        }
    }
    if !any && !text.trim().is_empty() {
        return Err("the report has no gcov JSON in it".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcov_of_node_and_jest_is_read() {
        let node = "TN:\nSF:n\\math.test.mjs\nFN:3,anonymous_0\nFNDA:1,anonymous_0\nDA:1,1\nDA:2,1\nDA:3,0\nDA:4,2\nLF:4\nLH:3\nend_of_record\nTN:\nSF:/p/lib.mjs\nDA:1,1\nend_of_record\n";
        let files = parse("lcov", node).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "n\\math.test.mjs");
        assert_eq!(files[0].lines, BTreeMap::from([(1, 1), (2, 1), (3, 0), (4, 2)]));
        assert_eq!(files[0].counts(), (3, 4));
        assert!((files[0].ratio() - 0.75).abs() < 1e-6);
        // a file twice (two test files used it) adds up
        let twice = parse("lcov", "SF:a.js\nDA:1,0\nDA:2,1\nend_of_record\nSF:a.js\nDA:1,3\nend_of_record\n").unwrap();
        assert_eq!(twice[0].lines, BTreeMap::from([(1, 3), (2, 1)]));
        assert_eq!(FileCoverage::default().ratio(), 1.0);
    }

    #[test]
    fn gcov_json_is_read_after_what_the_program_printed() {
        let out = "total 6\n{\"format_version\": \"1\", \"current_working_directory\": \"/mnt/e/p\", \"data_file\": \"prog.o\", \"files\": [{\"file\": \"prog.c\", \"lines\": [{\"line_number\": 2, \"count\": 3}, {\"line_number\": 9, \"count\": 0}]}, {\"file\": \"/usr/include/x.h\", \"lines\": []}]}\n";
        let files = parse("gcov-json", out).unwrap();
        assert_eq!(files[0].path, "/mnt/e/p/prog.c");
        assert_eq!(files[0].lines, BTreeMap::from([(2, 3), (9, 0)]));
        assert_eq!(files[1].path, "/usr/include/x.h");
        assert!(parse("gcov-json", "not json at all").is_err());
        assert!(parse("gcov-json", "").unwrap().is_empty());
        assert!(parse("xml", "").is_err());
    }
}

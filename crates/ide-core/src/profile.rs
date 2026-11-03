//! CPU profiles: where a program spent its time, as a tree of calls with the samples of each - read from the
//! `.cpuprofile` of Node.js (`node --cpu-prof`, Chrome's format) or from folded stacks (`main;parse;read 42`, what
//! `stackcollapse` scripts, py-spy's raw format and many profilers write).

/// A function in the tree of calls: the samples taken in it and in what it called.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub name: String,
    /// Where it is (`file:///...` or a path; empty for the runtime's own), and its 1-based line (0: unknown).
    pub file: String,
    pub line: usize,
    /// Samples in the function itself, and in it with all it called.
    pub own: u64,
    pub total: u64,
    pub children: Vec<Frame>,
}

/// A profile: the tree under a root that stands for the whole run, and how long a sample is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub root: Frame,
    /// Milliseconds a sample stands for (0: unknown).
    pub sample_ms: f64,
}

/// A function counted over the whole tree (it may be called from several places).
#[derive(Clone, Debug, PartialEq)]
pub struct Hot {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub own: u64,
    pub total: u64,
}

pub const FORMATS: &[&str] = &["cpuprofile", "folded"];

pub fn parse(format: &str, text: &str) -> Result<Profile, String> {
    match format {
        "cpuprofile" => parse_cpuprofile(text),
        "folded" => Ok(parse_folded(text)),
        other => Err(format!("unknown profile format {other:?}")),
    }
}

/// `node --cpu-prof`: the nodes of the call tree with their hit counts.
pub fn parse_cpuprofile(text: &str) -> Result<Profile, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not a CPU profile: {e}"))?;
    let nodes = v["nodes"].as_array().ok_or("not a CPU profile: no nodes")?;
    let by_id: std::collections::HashMap<i64, &serde_json::Value> = nodes.iter().filter_map(|n| Some((n["id"].as_i64()?, n))).collect();
    // hits from the samples when there are any (newer Node.js leaves hitCount out), else from hitCount
    let mut hits: std::collections::HashMap<i64, u64> = std::collections::HashMap::new();
    if let Some(samples) = v["samples"].as_array().filter(|s| !s.is_empty()) {
        for s in samples.iter().filter_map(|s| s.as_i64()) {
            *hits.entry(s).or_default() += 1;
        }
    } else {
        for n in nodes {
            if let (Some(id), Some(h)) = (n["id"].as_i64(), n["hitCount"].as_u64()) {
                hits.insert(id, h);
            }
        }
    }
    fn build(id: i64, by_id: &std::collections::HashMap<i64, &serde_json::Value>, hits: &std::collections::HashMap<i64, u64>, depth: usize) -> Frame {
        let n = by_id[&id];
        let cf = &n["callFrame"];
        let name = cf["functionName"].as_str().filter(|s| !s.is_empty()).unwrap_or("(anonymous)").to_string();
        let mut f = Frame { name, file: cf["url"].as_str().unwrap_or_default().to_string(), line: cf["lineNumber"].as_i64().map_or(0, |l| (l + 1).max(0) as usize), own: hits.get(&id).copied().unwrap_or(0), ..Default::default() };
        if depth < 400 {
            for c in n["children"].as_array().into_iter().flatten().filter_map(|c| c.as_i64()) {
                if by_id.contains_key(&c) {
                    f.children.push(build(c, by_id, hits, depth + 1));
                }
            }
        }
        f.total = f.own + f.children.iter().map(|c| c.total).sum::<u64>();
        f.children.sort_by_key(|c| std::cmp::Reverse(c.total));
        f
    }
    let root_id = nodes.first().and_then(|n| n["id"].as_i64()).ok_or("not a CPU profile: no root")?;
    let mut root = build(root_id, &by_id, &hits, 0);
    root.name = "all".into();
    // the time between samples, from the run's length
    let micros = v["endTime"].as_f64().unwrap_or(0.0) - v["startTime"].as_f64().unwrap_or(0.0);
    let sample_ms = if root.total > 0 && micros > 0.0 { micros / 1000.0 / root.total as f64 } else { 0.0 };
    Ok(Profile { root, sample_ms })
}

/// Folded stacks: `outer;inner;leaf COUNT` a line, the root first.
pub fn parse_folded(text: &str) -> Profile {
    let mut root = Frame { name: "all".into(), ..Default::default() };
    for line in text.lines() {
        let line = line.trim();
        let Some((stack, count)) = line.rsplit_once(' ') else { continue };
        let Ok(count) = count.trim().parse::<u64>() else { continue };
        let mut node = &mut root;
        node.total += count;
        for name in stack.split(';').filter(|s| !s.is_empty()) {
            let i = match node.children.iter().position(|c| c.name == name) {
                Some(i) => i,
                None => {
                    node.children.push(Frame { name: name.to_string(), ..Default::default() });
                    node.children.len() - 1
                }
            };
            node = &mut node.children[i];
            node.total += count;
        }
        node.own += count;
    }
    fn sort(f: &mut Frame) {
        f.children.sort_by_key(|c| std::cmp::Reverse(c.total));
        f.children.iter_mut().for_each(sort);
    }
    sort(&mut root);
    Profile { root, sample_ms: 0.0 }
}

/// The functions by the time spent in them, over the whole tree: (own samples first). A function in its own call
/// chain (recursion) counts its total once.
pub fn hot(root: &Frame) -> Vec<Hot> {
    fn walk(f: &Frame, stack: &mut Vec<(String, String, usize)>, out: &mut Vec<Hot>) {
        let key = (f.name.clone(), f.file.clone(), f.line);
        let recursive = stack.contains(&key);
        match out.iter_mut().find(|h| h.name == f.name && h.file == f.file && h.line == f.line) {
            Some(h) => {
                h.own += f.own;
                if !recursive {
                    h.total += f.total;
                }
            }
            None => out.push(Hot { name: f.name.clone(), file: f.file.clone(), line: f.line, own: f.own, total: f.total }),
        }
        stack.push(key);
        for c in &f.children {
            walk(c, stack, out);
        }
        stack.pop();
    }
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for c in &root.children {
        walk(c, &mut stack, &mut out);
    }
    out.sort_by_key(|h| std::cmp::Reverse((h.own, h.total)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE: &str = r#"{"nodes":[
        {"id":1,"callFrame":{"functionName":"(root)","url":"","lineNumber":-1},"children":[2,3]},
        {"id":2,"callFrame":{"functionName":"(program)","url":"","lineNumber":-1}},
        {"id":3,"callFrame":{"functionName":"main","url":"file:///p/p.js","lineNumber":2},"children":[4,5]},
        {"id":4,"callFrame":{"functionName":"fib","url":"file:///p/p.js","lineNumber":0},"children":[6]},
        {"id":5,"callFrame":{"functionName":"slow","url":"file:///p/p.js","lineNumber":1}},
        {"id":6,"callFrame":{"functionName":"fib","url":"file:///p/p.js","lineNumber":0}}],
        "startTime":0,"endTime":10000,"samples":[2,4,4,6,5,5,5,5,6,3],"timeDeltas":[1,1,1,1,1,1,1,1,1,1]}"#;

    #[test]
    fn a_node_profile_is_a_tree_of_calls_with_their_samples() {
        let p = parse("cpuprofile", NODE).unwrap();
        assert_eq!(p.root.total, 10);
        assert!((p.sample_ms - 1.0).abs() < 1e-9, "10 ms over 10 samples");
        let main = p.root.children.iter().find(|c| c.name == "main").unwrap();
        assert_eq!((main.total, main.own, main.line, main.file.as_str()), (9, 1, 3, "file:///p/p.js"));
        // fib (4, with its recursion) and slow (4); a tie keeps the order of the profile
        assert_eq!(main.children.iter().map(|c| (c.name.as_str(), c.total)).collect::<Vec<_>>(), [("fib", 4), ("slow", 4)]);
        let hot = hot(&p.root);
        assert_eq!(hot[..2].iter().map(|h| (h.name.as_str(), h.own)).collect::<Vec<_>>(), [("fib", 4), ("slow", 4)]);
        let fib = hot.iter().find(|h| h.name == "fib").unwrap();
        assert_eq!((fib.own, fib.total), (4, 4), "recursion counts its time once");
        assert!(parse("cpuprofile", "{}").is_err() && parse("cpuprofile", "nope").is_err());
    }

    #[test]
    fn folded_stacks_are_merged_into_a_tree() {
        let p = parse("folded", "main;parse;read 30\nmain;parse 10\nmain;render 50\n# comment\nbroken line\nidle 10\n").unwrap();
        assert_eq!(p.root.total, 100);
        let main = &p.root.children[0];
        assert_eq!((main.name.as_str(), main.total, main.own), ("main", 90, 0));
        assert_eq!((main.children[0].name.as_str(), main.children[0].total), ("render", 50));
        assert_eq!((main.children[1].total, main.children[1].own), (40, 10));
        assert_eq!(hot(&p.root)[0].name, "render");
        assert!(parse("xml", "").is_err());
    }
}

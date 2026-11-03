//! The containers of Docker as `docker ps --all --format '{{json .}}'` lists them: one JSON object a line.

/// A container.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`, `paused`, `restarting`, `created`, `dead`.
    pub state: String,
    /// `Up 5 minutes`, `Exited (0) 1 hour ago`.
    pub status: String,
    pub ports: String,
    /// The Compose project and service it belongs to (empty: started on its own).
    pub project: String,
    pub service: String,
}

impl Container {
    pub fn running(&self) -> bool {
        matches!(self.state.as_str(), "running" | "restarting")
    }
}

/// Read what `docker ps --all --format '{{json .}}'` printed. Lines that are not such JSON are skipped.
pub fn parse_ps(text: &str) -> Vec<Container> {
    text.lines().filter_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        let labels = s("Labels");
        let label = |name: &str| labels.split(',').find_map(|kv| kv.strip_prefix(name).and_then(|r| r.strip_prefix('='))).unwrap_or_default().to_string();
        let id = s("ID");
        (!id.is_empty()).then(|| Container {
            id,
            name: s("Names"),
            image: s("Image"),
            state: s("State").to_lowercase(),
            status: s("Status"),
            ports: s("Ports"),
            project: label("com.docker.compose.project"),
            service: label("com.docker.compose.service"),
        })
    }).collect()
}

/// The containers in groups: the Compose projects by name, then those on their own (an empty project name), each
/// group's running containers first.
pub fn grouped(containers: &[Container]) -> Vec<(String, Vec<Container>)> {
    let mut groups: Vec<(String, Vec<Container>)> = Vec::new();
    for c in containers {
        match groups.iter_mut().find(|(p, _)| *p == c.project) {
            Some((_, v)) => v.push(c.clone()),
            None => groups.push((c.project.clone(), vec![c.clone()])),
        }
    }
    groups.sort_by(|a, b| (a.0.is_empty(), &a.0).cmp(&(b.0.is_empty(), &b.0)));
    for (_, v) in &mut groups {
        v.sort_by(|a, b| (!a.running(), &a.service, &a.name).cmp(&(!b.running(), &b.service, &b.name)));
    }
    groups
}

/// Can a command line carry the name or id as it is?
pub fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containers_are_read_and_grouped_by_their_compose_project() {
        let out = "{\"ID\":\"b97a\",\"Names\":\"static_site\",\"Image\":\"main-web\",\"State\":\"exited\",\"Status\":\"Exited (0) 2 hours ago\",\"Ports\":\"\",\"Labels\":\"com.docker.compose.project=main,com.docker.compose.service=web,maintainer=NGINX\"}\n\
            not json\n\
            {\"ID\":\"a1\",\"Names\":\"api\",\"Image\":\"main-api\",\"State\":\"running\",\"Status\":\"Up 1 hour\",\"Ports\":\"0.0.0.0:80->80/tcp\",\"Labels\":\"com.docker.compose.project=main,com.docker.compose.service=api\"}\n\
            {\"ID\":\"c3\",\"Names\":\"alone\",\"Image\":\"busybox\",\"State\":\"Exited\",\"Status\":\"Exited (1)\",\"Labels\":\"\"}\n";
        let list = parse_ps(out);
        assert_eq!(list.len(), 3);
        assert_eq!((list[0].project.as_str(), list[0].service.as_str(), list[0].running()), ("main", "web", false));
        assert_eq!(list[2].state, "exited");
        let groups = grouped(&list);
        assert_eq!(groups.iter().map(|(p, v)| (p.as_str(), v.len())).collect::<Vec<_>>(), [("main", 2), ("", 1)]);
        assert_eq!(groups[0].1[0].name, "api", "the running one first");
        assert!(plain_name("demo-web-1") && plain_name("b97a233339cd") && !plain_name("a b") && !plain_name("x;rm"));
    }
}

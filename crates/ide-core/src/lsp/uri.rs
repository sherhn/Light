//! File names as the protocol writes them: `file://` URIs of the paths the server sees (`/home/me/a.c`, or `C:/a.c`
//! for a server that runs on Windows).

/// Bytes kept as they are in a URI path; everything else is written as `%XX`.
fn plain(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
}

/// The URI of a path as the server sees it: `/home/me/a b.c` -> `file:///home/me/a%20b.c`, `C:\src\a.c` ->
/// `file:///C:/src/a.c`, `//server/share/a.c` -> `file://server/share/a.c`.
pub fn from_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    let b = p.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    let mut out = String::from(if drive { "file:///" } else if p.starts_with("//") { "file:" } else { "file://" });
    for (i, &c) in b.iter().enumerate() {
        if plain(c) || (drive && i == 1) {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// The path of a `file://` URI, as the server wrote it but with `%XX` decoded: `/home/me/a.c`, `C:/a.c` (the drive
/// letter in upper case: `file:///c%3A/a.c` is `C:/a.c` too), `//server/share/a.c`. `None` for another scheme.
pub fn to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://").or_else(|| uri.strip_prefix("FILE://"))?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && let Some(v) = rest.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            bytes.push(v);
            i += 3;
            continue;
        }
        bytes.push(raw[i]);
        i += 1;
    }
    let s = String::from_utf8(bytes).ok()?;
    if !s.starts_with('/') {
        // `file://server/share/a.c`
        return Some(format!("//{s}"));
    }
    let b = s.as_bytes();
    if b.len() >= 3 && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return Some(format!("{}{}", (b[1] as char).to_ascii_uppercase(), &s[2..]));
    }
    Some(s)
}

/// What two URIs of one file have in common, however a server writes them (`file:///e%3A/a.c` and
/// `file:///E:/a.c` are one file; so are `C:/A.c` and `c:/a.c`, Windows ignoring case). For comparing only.
pub fn key(uri: &str) -> String {
    match to_path(uri) {
        Some(p) if p.as_bytes().get(1) == Some(&b':') || p.starts_with("//") => p.to_lowercase(),
        Some(p) => p,
        None => uri.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_paths() {
        assert_eq!(from_path("/home/me/my project/main.c"), "file:///home/me/my%20project/main.c");
        assert_eq!(to_path("file:///home/me/my%20project/main.c").unwrap(), "/home/me/my project/main.c");
        let odd = "/tmp/проект/a#b%c?.c";
        assert_eq!(to_path(&from_path(odd)).unwrap(), odd);
    }

    #[test]
    fn windows_paths() {
        assert_eq!(from_path(r"E:\Projects\demo\main.c"), "file:///E:/Projects/demo/main.c");
        assert_eq!(to_path("file:///E:/Projects/demo/main.c").unwrap(), "E:/Projects/demo/main.c");
        // the way VS Code writes them
        assert_eq!(to_path("file:///e%3A/Projects/demo/main.c").unwrap(), "E:/Projects/demo/main.c");
        assert_eq!(from_path(r"\\wsl.localhost\Ubuntu\home\a.c"), "file://wsl.localhost/Ubuntu/home/a.c");
        assert_eq!(to_path("file://wsl.localhost/Ubuntu/home/a.c").unwrap(), "//wsl.localhost/Ubuntu/home/a.c");
    }

    #[test]
    fn one_file_has_one_key() {
        assert_eq!(key("file:///e%3A/Projects/Demo/main.c"), key("file:///E:/Projects/Demo/main.c"));
        assert_eq!(key("file:///E:/Projects/Demo/main.c"), key("file:///e:/projects/demo/MAIN.c"));
        // Linux tells case apart
        assert_ne!(key("file:///home/me/A.c"), key("file:///home/me/a.c"));
        assert_eq!(key("file:///home/me/a%20b.c"), key("file:///home/me/a b.c"));
    }

    #[test]
    fn other_schemes_are_no_files() {
        assert!(to_path("untitled:Untitled-1").is_none());
        assert!(to_path("https://example.com/a.c").is_none());
    }
}

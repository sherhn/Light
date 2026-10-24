//! The little HTML that shows up in READMEs: entities, and tags that are turned into inlines (`<br>`, `<img>`, `<a>`,
//! `<b>`, `<code>`...) or dropped.

/// `&amp;`, `&#38;`, `&#x26;` -> the character, if it is one we know.
pub fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse::<u32>().ok()?,
        };
        return char::from_u32(code).filter(|c| *c != '\0');
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "hellip" => '\u{2026}',
        "mdash" => '\u{2014}',
        "ndash" => '\u{2013}',
        "laquo" => '\u{ab}',
        "raquo" => '\u{bb}',
        "middot" => '\u{b7}',
        "bull" => '\u{2022}',
        "rarr" => '\u{2192}',
        "larr" => '\u{2190}',
        "times" => '\u{d7}',
        "deg" => '\u{b0}',
        _ => return None,
    })
}

/// Replace every known entity in `s`.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let end = rest.find(';').filter(|e| *e <= 10);
        match end.and_then(|e| entity(&rest[1..e]).map(|c| (e, c))) {
            Some((e, c)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A parsed `<tag attr="v">`.
#[derive(Debug, PartialEq)]
pub struct Tag {
    /// Lower case; empty for a comment.
    pub name: String,
    pub closing: bool,
    pub attrs: Vec<(String, String)>,
    /// Length of the tag in chars, from `<` to `>`.
    pub len: usize,
}

impl Tag {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Parse a tag at the start of `chars` (which starts with `<`).
pub fn parse_tag(chars: &[char]) -> Option<Tag> {
    if chars.first() != Some(&'<') {
        return None;
    }
    // a comment
    if chars.starts_with(&['<', '!', '-', '-']) {
        let mut i = 4;
        while i + 2 < chars.len() {
            if chars[i] == '-' && chars[i + 1] == '-' && chars[i + 2] == '>' {
                return Some(Tag { name: String::new(), closing: false, attrs: vec![], len: i + 3 });
            }
            i += 1;
        }
        return None;
    }
    let mut i = 1;
    let closing = chars.get(i) == Some(&'/');
    if closing {
        i += 1;
    }
    let ns = i;
    while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '-') {
        i += 1;
    }
    if i == ns || !chars[ns].is_ascii_alphabetic() {
        return None;
    }
    let name: String = chars[ns..i].iter().collect::<String>().to_ascii_lowercase();
    let mut attrs = vec![];
    loop {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        match chars.get(i)? {
            '>' => return Some(Tag { name, closing, attrs, len: i + 1 }),
            '/' => i += 1,
            c if c.is_ascii_alphabetic() || *c == '_' || *c == ':' => {
                let s = i;
                while i < chars.len() && !chars[i].is_whitespace() && !matches!(chars[i], '=' | '>' | '/') {
                    i += 1;
                }
                let key: String = chars[s..i].iter().collect::<String>().to_ascii_lowercase();
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                let mut val = String::new();
                if chars.get(i) == Some(&'=') {
                    i += 1;
                    while i < chars.len() && chars[i].is_whitespace() {
                        i += 1;
                    }
                    match chars.get(i)? {
                        q @ ('"' | '\'') => {
                            let q = *q;
                            i += 1;
                            let s = i;
                            while i < chars.len() && chars[i] != q {
                                i += 1;
                            }
                            if i >= chars.len() {
                                return None;
                            }
                            val = chars[s..i].iter().collect();
                            i += 1;
                        }
                        _ => {
                            let s = i;
                            while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '>' {
                                i += 1;
                            }
                            val = chars[s..i].iter().collect();
                        }
                    }
                }
                attrs.push((key, decode_entities(&val)));
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> Option<Tag> {
        parse_tag(&s.chars().collect::<Vec<_>>())
    }

    #[test]
    fn entities_decode() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt; &#65;&#x42; &nope; &"), "a & b <c> AB &nope; &");
    }

    #[test]
    fn tags_parse() {
        let t = tag(r#"<img src="a b.png" alt='x' width=20 />rest"#).unwrap();
        assert_eq!((t.name.as_str(), t.closing, t.attr("src"), t.attr("alt"), t.attr("width")), ("img", false, Some("a b.png"), Some("x"), Some("20")));
        assert_eq!(t.len, r#"<img src="a b.png" alt='x' width=20 />"#.chars().count());
        assert!(tag("</a>").unwrap().closing);
        assert_eq!(tag("<!-- x -->").unwrap().len, 10);
        assert!(tag("<3 hearts>").is_none());
        assert!(tag("<a href=\"x").is_none());
    }
}

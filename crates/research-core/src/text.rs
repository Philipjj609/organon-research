//! HTML to readable text, plus what triage and independence need: title, publication date,
//! outbound links. Deliberately small: prefer `<article>`/`<main>` when they hold the content,
//! drop page furniture (nav, header, footer, aside, scripts), keep paragraph breaks.

use crate::url;

/// What a page says, extracted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extracted {
    pub title: String,
    pub text: String,
    /// Absolute outbound links, in order, without duplicates.
    pub links: Vec<String>,
    /// `YYYY-MM-DD`, from page metadata, if any.
    pub published: Option<String>,
}

/// Elements whose content is never the page's text.
const DROP: &[&str] = &[
    "script", "style", "noscript", "svg", "nav", "header", "footer", "aside", "form", "iframe",
    "template", "button",
];
/// Elements that end a line.
const BLOCK: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "tr",
    "section",
    "article",
    "main",
    "pre",
    "blockquote",
    "table",
    "ul",
    "ol",
    "dd",
    "dt",
    "figcaption",
    "hr",
];
/// Main content must be at least this long to be preferred over the whole body.
const MIN_MAIN_CHARS: usize = 400;

/// Extract a page. `base` resolves relative links.
#[must_use]
pub fn extract(html: &str, base: &str) -> Extracted {
    let title = meta(html, "og:title")
        .or_else(|| inner(html, "title").map(|t| decode(&t)))
        .unwrap_or_default();
    let published = [
        "article:published_time",
        "citation_publication_date",
        "citation_date",
        "dc.date",
        "date",
        "datePublished",
    ]
    .iter()
    .find_map(|k| meta(html, k))
    .or_else(|| json_ld_date(html))
    .or_else(|| attr_of_first(html, "time", "datetime"))
    .and_then(|d| date_prefix(&d));
    let body = ["article", "main"]
        .iter()
        .filter_map(|t| inner(html, t))
        .map(|h| to_text(&h))
        .find(|t| t.chars().count() >= MIN_MAIN_CHARS)
        .unwrap_or_else(|| to_text(inner(html, "body").as_deref().unwrap_or(html)));
    let mut links: Vec<String> = Vec::new();
    for href in attrs(html, "a", "href") {
        if let Some(l) = url::resolve(base, &decode(&href)) {
            if !links.contains(&l) {
                links.push(l);
            }
        }
    }
    Extracted {
        title: one_line(&title),
        text: body,
        links,
        published,
    }
}

/// Visible text of an HTML fragment.
#[must_use]
pub fn to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len() / 2);
    let mut i = 0;
    while i < html.len() {
        let Some(off) = html[i..].find('<') else {
            out.push_str(&html[i..]);
            break;
        };
        out.push_str(&html[i..i + off]);
        i += off;
        if lower[i..].starts_with("<!--") {
            i = lower[i..].find("-->").map_or(html.len(), |e| i + e + 3);
            continue;
        }
        let Some(end) = html[i..].find('>') else {
            break;
        };
        let tag = &lower[i + 1..i + end];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        i += end + 1;
        if !closing && DROP.contains(&name.as_str()) && !tag.ends_with('/') {
            let close = format!("</{name}");
            i = lower[i..].find(&close).map_or(html.len(), |e| {
                let after = i + e;
                lower[after..]
                    .find('>')
                    .map_or(html.len(), |g| after + g + 1)
            });
            continue;
        }
        if BLOCK.contains(&name.as_str()) {
            out.push('\n');
        } else if name == "td" || name == "th" {
            out.push(' ');
        }
    }
    tidy(&decode(&out))
}

/// Collapse spaces within lines, drop empty lines beyond one.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        out.push_str(&line);
        blank = 0;
    }
    out
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decode the HTML entities that occur in practice.
#[must_use]
pub fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // Bytes, not a str slice: 12 bytes may end inside a multi-byte character.
        let Some(semi) = rest.bytes().take(12).position(|b| b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "rsquo" | "lsquo" => Some('\''),
            "ldquo" | "rdquo" => Some('"'),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
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

/// Inner HTML of the first `<tag …>…</tag>` (case-insensitive).
fn inner(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = find_open(&lower, tag, 0)?;
    let start = open + lower[open..].find('>')? + 1;
    let end = lower[start..]
        .find(&format!("</{tag}"))
        .map_or(html.len(), |e| start + e);
    Some(html[start..end].to_owned())
}

/// Position of `<tag` followed by a delimiter (so `<a` does not match `<article`).
fn find_open(lower: &str, tag: &str, from: usize) -> Option<usize> {
    let needle = format!("<{tag}");
    let mut at = from;
    while let Some(i) = lower[at..].find(&needle) {
        let pos = at + i;
        let next = lower[pos + needle.len()..].chars().next();
        if matches!(next, Some(' ' | '>' | '\n' | '\t' | '\r' | '/')) {
            return Some(pos);
        }
        at = pos + needle.len();
    }
    None
}

/// Values of `name` on every `<tag …>`.
fn attrs(html: &str, tag: &str, name: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(pos) = find_open(&lower, tag, at) {
        let Some(end) = lower[pos..].find('>') else {
            break;
        };
        if let Some(v) = attribute(&html[pos..pos + end], name) {
            out.push(v);
        }
        at = pos + end;
    }
    out
}

fn attr_of_first(html: &str, tag: &str, name: &str) -> Option<String> {
    attrs(html, tag, name).into_iter().next()
}

/// Value of attribute `name` in one tag's source.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut at = 0;
    while let Some(i) = lower[at..].find(name) {
        let pos = at + i;
        let before_ok = pos == 0 || lower.as_bytes()[pos - 1].is_ascii_whitespace();
        let rest = lower[pos + name.len()..].trim_start();
        if before_ok && rest.starts_with('=') {
            let vstart = tag.len() - rest.len() + 1;
            let v = tag[vstart..].trim_start();
            let (q, body) = match v.chars().next() {
                Some(q @ ('"' | '\'')) => (Some(q), &v[1..]),
                _ => (None, v),
            };
            let end = match q {
                Some(q) => body.find(q),
                None => body.find(|c: char| c.is_whitespace() || c == '>'),
            }
            .unwrap_or(body.len());
            return Some(body[..end].to_owned());
        }
        at = pos + name.len();
    }
    None
}

/// `<meta property|name="key" content="…">`.
fn meta(html: &str, key: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let key = key.to_ascii_lowercase();
    let mut at = 0;
    while let Some(pos) = find_open(&lower, "meta", at) {
        let end = pos + lower[pos..].find('>')?;
        let tag = &html[pos..end];
        let named = ["property", "name", "itemprop"]
            .iter()
            .any(|a| attribute(tag, a).is_some_and(|v| v.to_ascii_lowercase() == key));
        if named {
            if let Some(c) = attribute(tag, "content").filter(|c| !c.trim().is_empty()) {
                return Some(decode(c.trim()));
            }
        }
        at = end;
    }
    None
}

fn json_ld_date(html: &str) -> Option<String> {
    let i = html.find("\"datePublished\"")?;
    let rest = &html[i + "\"datePublished\"".len()..];
    let start = rest.find('"')? + 1;
    let end = start + rest[start..].find('"')?;
    Some(rest[start..end].to_owned())
}

/// `YYYY-MM-DD` from the start of a date string (`2023/05/01`, `2023-05-01T…`).
fn date_prefix(s: &str) -> Option<String> {
    let d: String = s
        .trim()
        .chars()
        .take(10)
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    let b = d.as_bytes();
    let ok = d.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && d.chars()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if ok {
        return Some(d);
    }
    // A bare year is still worth knowing.
    let y: String = s.trim().chars().take(4).collect();
    (y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()) && s.trim().len() == 4)
        .then(|| format!("{y}-01-01"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = concat!(
        r#"<!doctype html><html><head><title>Ignored &amp; plain</title>
<meta property="og:title" content="Why X &mdash; a study">
<meta name="citation_publication_date" content="2023/05/01">
<script>var x = "<p>not text</p>";</script><style>p{color:red}</style></head>
<body><nav><a href="/home">Home</a> Menu</nav><header>Site header</header>
<article><h1>Why X</h1><p>First paragraph says   something&nbsp;important.</p>
<p>Second cites <a href="https://other.org/origin?utm_source=a">the origin</a> and <a href='/local'>a local page</a>.</p>
<!-- <p>commented out</p> -->
<p>"#,
        "Padding sentence to make the article long enough to be preferred over the body. ",
        "Padding sentence to make the article long enough to be preferred over the body. ",
        "Padding sentence to make the article long enough to be preferred over the body. ",
        "Padding sentence to make the article long enough to be preferred over the body. ",
        r#"Padding.</p></article><footer>Copyright</footer></body></html>"#
    );

    #[test]
    fn article_text_without_furniture_with_links_and_date() {
        let e = extract(PAGE, "https://site.com/post/1");
        assert_eq!(e.title, "Why X — a study");
        assert_eq!(e.published.as_deref(), Some("2023-05-01"));
        assert!(
            e.text
                .starts_with("Why X\n\nFirst paragraph says something important."),
            "{}",
            e.text
        );
        for absent in [
            "not text",
            "color:red",
            "Menu",
            "Site header",
            "Copyright",
            "commented out",
        ] {
            assert!(!e.text.contains(absent), "{absent} leaked: {}", e.text);
        }
        assert_eq!(
            e.links,
            [
                "https://site.com/home",
                "https://other.org/origin?utm_source=a",
                "https://site.com/local"
            ]
        );
    }

    #[test]
    fn entities_decode_and_unknown_ones_survive() {
        assert_eq!(
            decode("a &amp; b &#8212; c &#x2019; &bogus; d &"),
            "a & b — c ’ &bogus; d &"
        );
    }

    #[test]
    fn an_ampersand_before_multibyte_text_does_not_panic() {
        // Regression: the entity window ended inside '│' and panicked.
        assert_eq!(decode("&ab ──│ x;"), "&ab ──│ x;");
        assert_eq!(decode("R&Dx ──│ &amp;"), "R&Dx ──│ &");
    }

    #[test]
    fn short_main_falls_back_to_body() {
        let e = extract(
            "<body><main>tiny</main><p>The real text lives outside main.</p></body>",
            "https://a.com",
        );
        assert!(e.text.contains("The real text"), "{}", e.text);
    }
}

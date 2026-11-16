//! URLs: one canonical form per page (so the same page found twice is one source), and the
//! *origin* that decides independence (two pages from one origin are one voice).

/// Query parameters that track the visitor and never change what a page says.
fn is_tracking(key: &str) -> bool {
    key.starts_with("utm_")
        || matches!(
            key,
            "fbclid" | "gclid" | "mc_cid" | "mc_eid" | "ref_src" | "igshid" | "si"
        )
}

/// Parts of an absolute http(s) URL. `None` for anything else.
struct Parts<'a> {
    host: String,
    path: &'a str,
    query: &'a str,
}

fn parts(url: &str) -> Option<Parts<'_>> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let rest = rest.split('#').next().unwrap_or_default();
    let host_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let host = rest[..host_end].to_lowercase();
    let host = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("m."))
        .unwrap_or(&host)
        .to_owned();
    let host = if host == "dx.doi.org" {
        "doi.org".to_owned()
    } else {
        host
    };
    if host.is_empty() {
        return None;
    }
    let after = &rest[host_end..];
    let (path, query) = after.split_once('?').unwrap_or((after, ""));
    Some(Parts { host, path, query })
}

/// Canonical form: https, lowercase host without `www.`/`m.`, no fragment, no tracking
/// parameters, remaining parameters sorted, no trailing slash, arXiv PDFs and versions folded
/// into the abstract page. `None` if not an absolute http(s) URL.
#[must_use]
pub fn canonical(url: &str) -> Option<String> {
    let Parts { host, path, query } = parts(url)?;
    let mut path = path.trim_end_matches('/').to_owned();
    if host == "arxiv.org" {
        if let Some(id) = path
            .strip_prefix("/pdf/")
            .or_else(|| path.strip_prefix("/abs/"))
            .or_else(|| path.strip_prefix("/html/"))
        {
            let id = id.trim_end_matches(".pdf");
            let id = strip_arxiv_version(id);
            path = format!("/abs/{id}");
        }
    }
    let mut params: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty() && !is_tracking(p.split('=').next().unwrap_or_default()))
        .collect();
    params.sort_unstable();
    let query = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    Some(format!("https://{host}{path}{query}"))
}

fn strip_arxiv_version(id: &str) -> &str {
    match id.rfind('v') {
        Some(i) if i > 0 && id[i + 1..].chars().all(|c| c.is_ascii_digit()) && i + 1 < id.len() => {
            &id[..i]
        }
        _ => id,
    }
}

/// The arXiv id of an arXiv URL (`2605.06635`).
#[must_use]
pub fn arxiv_id(url: &str) -> Option<String> {
    let c = canonical(url)?;
    c.strip_prefix("https://arxiv.org/abs/").map(str::to_owned)
}

/// Lowercase host without `www.`/`m.`.
#[must_use]
pub fn host(url: &str) -> Option<String> {
    parts(url).map(|p| p.host)
}

/// Registrable domain, approximately: the last two labels, or three under a two-letter
/// country code with a generic second level (`bbc.co.uk`, `ox.ac.uk`).
#[must_use]
pub fn site(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    let take = if n >= 3
        && labels[n - 1].len() == 2
        && matches!(
            labels[n - 2],
            "co" | "com" | "ac" | "gov" | "org" | "net" | "edu"
        ) {
        3
    } else {
        2
    };
    labels[n.saturating_sub(take)..].join(".")
}

/// Hosts where each document stands alone (papers, threads): origin = the document.
const DOCUMENT_HOSTS: &[&str] = &[
    "arxiv.org",
    "doi.org",
    "openreview.net",
    "aclanthology.org",
    "openalex.org",
    "semanticscholar.org",
    "news.ycombinator.com",
    "reddit.com",
    "old.reddit.com",
    "lobste.rs",
    "stackoverflow.com",
];
/// Hosts where the author is the first path segment: origin = host + author.
const AUTHOR_PATH_HOSTS: &[&str] = &[
    "github.com",
    "gitlab.com",
    "medium.com",
    "x.com",
    "twitter.com",
    "youtube.com",
    "huggingface.co",
    "dev.to",
];
/// Hosts whose subdomains are separate authors: origin = the full host.
const TENANT_SITES: &[&str] = &[
    "substack.com",
    "github.io",
    "medium.com",
    "blogspot.com",
    "wordpress.com",
    "netlify.app",
    "vercel.app",
    "pages.dev",
    "hashnode.dev",
    "bearblog.dev",
];

/// Who is speaking: pages with the same origin are not independent of each other.
#[must_use]
pub fn origin(url: &str) -> Option<String> {
    let c = canonical(url)?;
    let p = parts(&c)?;
    if DOCUMENT_HOSTS.contains(&p.host.as_str()) {
        return Some(c);
    }
    if let Some(name) = crate_name(&p.host, p.path) {
        return Some(format!("crates.io/{name}"));
    }
    // Raw files speak for the GitHub account that owns them.
    let host = if p.host == "raw.githubusercontent.com" {
        "github.com".to_owned()
    } else {
        p.host
    };
    if AUTHOR_PATH_HOSTS.contains(&host.as_str()) {
        let author = p
            .path
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or_default();
        return Some(if author.is_empty() {
            host
        } else {
            format!("{host}/{}", author.to_lowercase())
        });
    }
    let s = site(&host);
    if TENANT_SITES.contains(&s.as_str()) && host != s {
        return Some(host);
    }
    Some(s)
}

/// The Rust crate a docs.rs or crates.io page is about, normalized (crates.io treats `-` and `_`
/// as the same name). Each crate is its own author's voice, on either site; `None` for the sites'
/// own pages (about, releases, users).
fn crate_name(host: &str, path: &str) -> Option<String> {
    let mut seg = path.split('/').filter(|s| !s.is_empty());
    let name = match host {
        // docs.rs/<crate>/…, docs.rs/crate/<crate>/…
        "docs.rs" => match seg.next()? {
            "crate" => seg.next()?,
            "about" | "releases" | "-" | "static.files" => return None,
            name => name,
        },
        // crates.io/crates/<crate>, crates.io/api/v1/crates/<crate>
        "crates.io" => match seg.next()? {
            "crates" => seg.next()?,
            "api" => seg.find(|&s| s == "crates").and_then(|_| seg.next())?,
            _ => return None,
        },
        _ => return None,
    };
    Some(name.to_lowercase().replace('_', "-"))
}

/// Resolve `href` against the page it appears on (absolute and root-relative links only).
#[must_use]
pub fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_owned());
    }
    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if href.starts_with('/') {
        let p = parts(base)?;
        return Some(format!("https://{}{href}", p.host));
    }
    None
}

/// Percent-encode a query value.
#[must_use]
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b))
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_page_has_one_canonical_form() {
        let a = canonical("http://www.Example.com/post/?utm_source=x&b=2&a=1#top").unwrap();
        let b = canonical("https://example.com/post?a=1&b=2&fbclid=zz").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, "https://example.com/post?a=1&b=2");
    }

    #[test]
    fn arxiv_pdfs_html_and_versions_fold_into_the_abstract() {
        for u in [
            "https://arxiv.org/pdf/2605.06635v3.pdf",
            "https://arxiv.org/abs/2605.06635",
            "https://arxiv.org/html/2605.06635v1",
        ] {
            assert_eq!(
                canonical(u).unwrap(),
                "https://arxiv.org/abs/2605.06635",
                "{u}"
            );
        }
        assert_eq!(
            arxiv_id("https://arxiv.org/pdf/2605.06635v2").as_deref(),
            Some("2605.06635")
        );
    }

    #[test]
    fn origins_follow_who_speaks() {
        let o = |u: &str| origin(u).unwrap();
        assert_eq!(
            o("https://blog.example.com/a"),
            o("https://www.example.com/b"),
            "one company, one voice"
        );
        assert_ne!(
            o("https://alice.substack.com/p/x"),
            o("https://bob.substack.com/p/y"),
            "tenants are separate authors"
        );
        assert_ne!(
            o("https://github.com/alice/x"),
            o("https://github.com/bob/y")
        );
        assert_eq!(
            o("https://github.com/alice/x"),
            o("https://github.com/Alice/z")
        );
        // Regression: two owners' raw files were one voice ("githubusercontent.com").
        assert_ne!(
            o("https://raw.githubusercontent.com/paperclipai/paperclip/master/README.md"),
            o("https://raw.githubusercontent.com/open-telemetry/semconv/main/a.md")
        );
        assert_eq!(
            o("https://raw.githubusercontent.com/alice/x/main/README.md"),
            o("https://github.com/alice/x")
        );
        assert_ne!(
            o("https://arxiv.org/abs/1"),
            o("https://arxiv.org/abs/2"),
            "each paper stands alone"
        );
        assert_eq!(site("news.bbc.co.uk"), "bbc.co.uk");
    }

    // Regression (eval 2026-11-16, q19): docs.rs pages of two crates were one voice ("docs.rs").
    #[test]
    fn each_crate_is_its_own_voice() {
        let o = |u: &str| origin(u).unwrap();
        assert_ne!(
            o("https://docs.rs/crate/pyannote-rs/0.3.4"),
            o("https://docs.rs/crate/speakrs/0.2.1/source/README.md")
        );
        assert_eq!(
            o("https://docs.rs/crate/speakrs/0.2.1"),
            "crates.io/speakrs"
        );
        for u in [
            "https://docs.rs/speakrs/latest/speakrs/struct.Pipeline.html",
            "https://docs.rs/Speakrs",
            "https://crates.io/crates/speakrs",
            "https://crates.io/api/v1/crates/speakrs/0.2.1/download",
        ] {
            assert_eq!(o(u), "crates.io/speakrs", "{u}");
        }
        assert_eq!(
            o("https://docs.rs/pyannote_rs"),
            o("https://crates.io/crates/pyannote-rs")
        );
        assert_eq!(o("https://docs.rs/about/badges"), "docs.rs");
        assert_eq!(o("https://docs.rs"), "docs.rs");
        assert_eq!(o("https://crates.io/users/someone"), "crates.io");
    }

    #[test]
    fn links_resolve_and_queries_encode() {
        assert_eq!(
            resolve("https://a.com/x/y", "/z").as_deref(),
            Some("https://a.com/z")
        );
        assert_eq!(resolve("https://a.com/x", "#frag"), None);
        assert_eq!(encode("C++ vs Rust?"), "C%2B%2B+vs+Rust%3F");
    }
}

//! Source triage: what kind of voice a source is, and why. A hint for reading order, with its
//! reasons spelled out, never a verdict on truth. (Research agents left alone prefer SEO
//! farms over primary sources; this pushes the other way, visibly.)

use serde::{Deserialize, Serialize};

use crate::url;

/// Kind of voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Papers, standards, official documentation, the code itself, government and university.
    Primary,
    /// Encyclopedic summaries (good map, second-hand facts).
    Reference,
    /// People who did the thing: engineering blogs, talks, personal sites.
    Practitioner,
    /// Threads and comments: leads and dissent, not facts.
    Discussion,
    /// Listicles, roundups, affiliate pages: read last, cite never if a primary exists.
    Derivative,
    /// Nothing known either way.
    Unknown,
}

impl Class {
    /// Reading-order weight (higher first).
    #[must_use]
    pub fn weight(self) -> f64 {
        match self {
            Self::Primary => 3.0,
            Self::Practitioner => 2.0,
            Self::Reference => 1.5,
            Self::Unknown => 1.0,
            Self::Discussion => 0.8,
            Self::Derivative => 0.2,
        }
    }
}

/// A classification with its reasons.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Triage {
    pub class: Class,
    pub reasons: Vec<String>,
}

const PRIMARY_HOSTS: &[&str] = &[
    "arxiv.org",
    "doi.org",
    "openreview.net",
    "aclanthology.org",
    "dl.acm.org",
    "ieeexplore.ieee.org",
    "nature.com",
    "science.org",
    "w3.org",
    "rfc-editor.org",
    "datatracker.ietf.org",
    "github.com",
    "raw.githubusercontent.com",
    "gitlab.com",
    "pubmed.ncbi.nlm.nih.gov",
    "ncbi.nlm.nih.gov",
    "biorxiv.org",
    "medrxiv.org",
    "ssrn.com",
    "openalex.org",
    "crates.io",
    "docs.rs",
    "pypi.org",
    "npmjs.com",
];
const REFERENCE_SITES: &[&str] = &["wikipedia.org", "britannica.com"];
const DISCUSSION_HOSTS: &[&str] = &[
    "news.ycombinator.com",
    "reddit.com",
    "old.reddit.com",
    "lobste.rs",
    "x.com",
    "twitter.com",
    "stackoverflow.com",
    "quora.com",
];
const PRACTITIONER_HOSTS: &[&str] = &[
    "medium.com",
    "dev.to",
    "youtube.com",
    "hashnode.dev",
    "substack.com",
];

/// Classify a page from its URL, title, and outbound links.
#[must_use]
pub fn classify(page_url: &str, title: &str, links: &[String]) -> Triage {
    let host = url::host(page_url).unwrap_or_default();
    let site = url::site(&host);
    let path = page_url
        .split_once(&host)
        .map_or("", |(_, p)| p)
        .to_lowercase();
    let title_l = title.to_lowercase();
    let mut reasons = Vec::new();
    let tld = host.rsplit('.').next().unwrap_or_default();

    // Derivative signals first: a listicle on a "primary-looking" host is still a listicle.
    let affiliate = links.iter().filter(|l| is_affiliate(l)).count();
    let listicle = is_listicle(&title_l);
    if listicle {
        reasons.push(format!("listicle title \"{title}\""));
    }
    if affiliate >= 3 {
        reasons.push(format!("{affiliate} affiliate links"));
    }
    if listicle || affiliate >= 3 {
        return Triage {
            class: Class::Derivative,
            reasons,
        };
    }

    let class = if PRIMARY_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    {
        reasons.push(format!("primary host {host}"));
        Class::Primary
    } else if matches!(tld, "gov" | "edu" | "mil")
        || host.contains(".gov.")
        || host.contains(".ac.")
        || host.contains(".edu.")
    {
        reasons.push(format!("institutional domain {host}"));
        Class::Primary
    } else if host.starts_with("docs.")
        || host.starts_with("developer.")
        || host.starts_with("developers.")
        || path.starts_with("/docs")
    {
        reasons.push("official documentation".into());
        Class::Primary
    } else if REFERENCE_SITES.contains(&site.as_str()) {
        reasons.push(format!("reference work {site}"));
        Class::Reference
    } else if DISCUSSION_HOSTS.contains(&host.as_str()) {
        reasons.push(format!("discussion on {host}"));
        Class::Discussion
    } else if PRACTITIONER_HOSTS.contains(&site.as_str())
        || path.contains("/blog")
        || path.contains("/posts/")
        || host.starts_with("blog.")
        || host.starts_with("engineering.")
    {
        reasons.push("blog or talk".into());
        Class::Practitioner
    } else {
        Class::Unknown
    };
    Triage { class, reasons }
}

fn is_listicle(title: &str) -> bool {
    let starts_top = title.starts_with("top ")
        && title
            .split_whitespace()
            .nth(1)
            .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()));
    let best_year = (title.contains("best ") || title.starts_with("top "))
        && title
            .split(|c: char| !c.is_ascii_digit())
            .any(|n| n.len() == 4 && n.starts_with("20"));
    let numbered_best = title
        .split_whitespace()
        .next()
        .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()))
        && (title.contains(" best ") || title.contains(" top "));
    starts_top || best_year || numbered_best || title.contains("ultimate guide")
}

fn is_affiliate(link: &str) -> bool {
    let l = link.to_lowercase();
    (l.contains("amazon.") && (l.contains("tag=") || l.contains("/dp/")))
        || l.contains("affiliate")
        || l.contains("?ref=")
        || l.contains("&ref=")
        || l.contains("shareasale")
        || l.contains("impact.com")
        || l.contains("partnerize")
}

/// Expected value of reading a source next: class weight, plus one point per extra query that
/// surfaced it (found from several angles), minus a little for age.
#[must_use]
pub fn priority(
    class: Class,
    times_found: usize,
    published: Option<&str>,
    current_year: i32,
) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let found = times_found.saturating_sub(1) as f64;
    let age = published
        .and_then(|d| d.get(..4)?.parse::<i32>().ok())
        .map_or(0.0, |y| f64::from((current_year - y).clamp(0, 10)) * 0.05);
    class.weight() + found - age
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(u: &str, title: &str) -> Class {
        classify(u, title, &[]).class
    }

    #[test]
    fn voices_by_host_and_path() {
        assert_eq!(
            class("https://arxiv.org/abs/2605.06635", "Cited but not verified"),
            Class::Primary
        );
        assert_eq!(class("https://docs.rs/serde", "serde"), Class::Primary);
        assert_eq!(class("https://www.cs.ox.ac.uk/paper", "x"), Class::Primary);
        assert_eq!(
            class("https://en.wikipedia.org/wiki/SOLID", "SOLID"),
            Class::Reference
        );
        assert_eq!(
            class("https://news.ycombinator.com/item?id=1", "Show HN"),
            Class::Discussion
        );
        assert_eq!(
            class(
                "https://engineering.example.com/how-we-did-it",
                "How we did it"
            ),
            Class::Practitioner
        );
        assert_eq!(class("https://example.com/about", "About"), Class::Unknown);
        // Regression: a raw README was Unknown while its repo page was Primary.
        assert_eq!(
            class(
                "https://raw.githubusercontent.com/a/b/main/README.md",
                "README"
            ),
            Class::Primary
        );
    }

    #[test]
    fn listicles_and_affiliate_pages_are_derivative_wherever_they_live() {
        assert_eq!(
            class("https://example.com/x", "Top 10 Research Agents in 2026"),
            Class::Derivative
        );
        assert_eq!(
            class(
                "https://example.com/x",
                "The 7 best AI tools for research (2026)"
            ),
            Class::Derivative
        );
        assert_eq!(
            class("https://example.com/x", "The ultimate guide to RAG"),
            Class::Derivative
        );
        let links: Vec<String> = (0..3)
            .map(|i| format!("https://amazon.com/dp/B0{i}?tag=x-20"))
            .collect();
        let t = classify("https://example.com/x", "Some review", &links);
        assert_eq!(t.class, Class::Derivative);
        assert!(t.reasons[0].contains("affiliate"));
        assert_eq!(
            class("https://example.com/x", "Top-down design in 2026"),
            Class::Unknown,
            "not every 'top' is a listicle"
        );
    }

    #[test]
    fn primary_first_corroborated_up_old_down() {
        assert!(
            priority(Class::Primary, 1, None, 2026) > priority(Class::Practitioner, 1, None, 2026)
        );
        assert!(
            priority(Class::Practitioner, 3, None, 2026) > priority(Class::Primary, 1, None, 2026)
        );
        assert!(
            priority(Class::Primary, 1, Some("2016-01-01"), 2026)
                < priority(Class::Primary, 1, Some("2026-01-01"), 2026)
        );
    }
}

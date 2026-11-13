//! Independence: many pages repeating one article are one voice, not corroboration.
//!
//! Two sources are joined into one group when any of these holds:
//! - **same origin** (one company, one author: see [`crate::url::origin`]);
//! - **one cites the other** (links to its canonical URL): the citer is downstream;
//! - **copied text**: a large share of one's word 5-grams appears in the other (containment, so
//!   a short post lifting part of a long original is caught, which Jaccard would miss).
//!
//! Paraphrase without links or shared wording is not detected (stated limit): corroboration is
//! an upper bound on independence, never a proof of it.

use std::collections::HashSet;

use crate::matching::words;

/// Word n-gram size for copy detection.
pub const SHINGLE: usize = 5;
/// Share of the smaller text's shingles found in the other that marks a copy.
pub const COPY_CONTAINMENT: f64 = 0.5;

/// Hashed word 5-grams of a text (FNV-1a, stable across runs and platforms).
#[must_use]
pub fn shingles(text: &str) -> HashSet<u64> {
    let w: Vec<String> = words(text).into_iter().map(|w| w.norm).collect();
    w.windows(SHINGLE).map(|g| fnv(&g.join(" "))).collect()
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Share of the smaller set contained in the larger.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn containment(a: &HashSet<u64>, b: &HashSet<u64>) -> f64 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if small.is_empty() {
        return 0.0;
    }
    small.iter().filter(|h| large.contains(h)).count() as f64 / small.len() as f64
}

/// What one source needs for grouping.
#[derive(Debug, Clone)]
pub struct Item<'a> {
    pub canonical: &'a str,
    pub origin: &'a str,
    /// Canonical URLs it links to.
    pub links: &'a [String],
    pub shingles: &'a HashSet<u64>,
}

/// Why two sources were joined.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    SameOrigin,
    Cites,
    Copies(f64),
}

/// Group sources. Returns, for each item, the index of its group's first member, plus every
/// join with its reason (for the report: corroboration must be explainable).
#[must_use]
pub fn groups(items: &[Item<'_>]) -> (Vec<usize>, Vec<(usize, usize, Link)>) {
    let mut parent: Vec<usize> = (0..items.len()).collect();
    let mut joins = Vec::new();
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            let (a, b) = (&items[i], &items[j]);
            let why = if a.origin == b.origin {
                Some(Link::SameOrigin)
            } else if a.links.iter().any(|l| l == b.canonical)
                || b.links.iter().any(|l| l == a.canonical)
            {
                Some(Link::Cites)
            } else {
                let c = containment(a.shingles, b.shingles);
                (c >= COPY_CONTAINMENT).then_some(Link::Copies((c * 100.0).round() / 100.0))
            };
            if let Some(why) = why {
                union(&mut parent, i, j);
                joins.push((i, j, why));
            }
        }
    }
    let roots: Vec<usize> = (0..items.len()).map(|i| find(&mut parent, i)).collect();
    // Name each group by its first member.
    let mut first = vec![usize::MAX; items.len()];
    for (i, &r) in roots.iter().enumerate() {
        if first[r] == usize::MAX {
            first[r] = i;
        }
    }
    (roots.iter().map(|&r| first[r]).collect(), joins)
}

fn find(parent: &mut [usize], i: usize) -> usize {
    let mut r = i;
    while parent[r] != r {
        r = parent[r];
    }
    let mut i = i;
    while parent[i] != r {
        let next = parent[i];
        parent[i] = r;
        i = next;
    }
    r
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[ra.max(rb)] = ra.min(rb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = "Dependency inversion means that high level policy should not depend on low level detail; both should depend on abstractions owned by the policy. This stops the fan out of the policy module from growing as new devices are added, and so it stops the code from rotting over the years.";

    #[test]
    fn copies_citations_and_origins_join_independent_sources_stay_apart() {
        let copy = format!("Great read today. {} Thanks for sharing!", &ORIGINAL[..150]);
        let other = "Ousterhout argues for deep modules with simple interfaces; splitting code into many tiny classes raises total complexity.";
        let texts = [
            ORIGINAL,
            copy.as_str(),
            "A summary with a link.",
            other,
            "Another page from the same company blog.",
        ];
        let sh: Vec<HashSet<u64>> = texts.iter().map(|t| shingles(t)).collect();
        let none: Vec<String> = Vec::new();
        let cites = vec!["https://a.com/original".to_owned()];
        let items = [
            Item {
                canonical: "https://a.com/original",
                origin: "a.com",
                links: &none,
                shingles: &sh[0],
            },
            Item {
                canonical: "https://farm.net/x",
                origin: "farm.net",
                links: &none,
                shingles: &sh[1],
            },
            Item {
                canonical: "https://news.org/y",
                origin: "news.org",
                links: &cites,
                shingles: &sh[2],
            },
            Item {
                canonical: "https://book.org/z",
                origin: "book.org",
                links: &none,
                shingles: &sh[3],
            },
            Item {
                canonical: "https://a.com/other",
                origin: "a.com",
                links: &none,
                shingles: &sh[4],
            },
        ];
        let (g, joins) = groups(&items);
        assert_eq!(g, [0, 0, 0, 3, 0]);
        assert!(joins
            .iter()
            .any(|(i, j, l)| (*i, *j) == (0, 1) && matches!(l, Link::Copies(c) if *c >= 0.5)));
        assert!(joins.contains(&(0, 2, Link::Cites)));
        assert!(joins.contains(&(0, 4, Link::SameOrigin)));
    }

    #[test]
    fn shingles_are_stable() {
        assert_eq!(fnv("a b c d e"), fnv("a b c d e"));
        assert_eq!(shingles("One two three four five six").len(), 2);
    }
}

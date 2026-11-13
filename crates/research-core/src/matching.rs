//! Passage verification: does the quoted passage actually occur in the source's text, and where?
//!
//! This is the check research agents skip: audits find cited pages that exist but do not say
//! what the citation claims. Matching is on words (case, punctuation, quotes, dashes, line
//! breaks and markup differences are ignored) and order-aware (a longest common subsequence),
//! so a lightly reformatted quote verifies while a paraphrase or an invention does not.

use serde::{Deserialize, Serialize};

/// A quote shorter than this cannot carry a claim on its own.
pub const MIN_WORDS: usize = 6;
/// Share of the passage's words found in order for a close match.
pub const CLOSE: f64 = 0.85;
/// Below this, the passage is not there.
pub const PARTIAL: f64 = 0.5;

/// One word of a text with its character range in the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub norm: String,
    pub start: usize,
    pub end: usize,
}

/// Words of `text`: maximal runs of letters and digits, lowercased, with char offsets.
#[must_use]
pub fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut start = 0;
    let mut n = 0;
    for (i, c) in text.chars().enumerate() {
        n = i + 1;
        if c.is_alphanumeric() {
            if current.is_empty() {
                start = i;
            }
            current.extend(c.to_lowercase());
        } else if !current.is_empty() {
            out.push(Word {
                norm: std::mem::take(&mut current),
                start,
                end: i,
            });
        }
    }
    if !current.is_empty() {
        out.push(Word {
            norm: current,
            start,
            end: n,
        });
    }
    out
}

/// How well a passage was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every word, in order, contiguous.
    Exact,
    /// At least [`CLOSE`] of the words in order within a nearby span (reformatting, an elision).
    Close,
    /// Some overlap, not enough to say the source says this.
    Partial,
    /// Not in the text.
    Missing,
    /// Too short to verify a claim (fewer than [`MIN_WORDS`] words).
    TooShort,
}

impl Verdict {
    /// Exact and close matches count as verified.
    #[must_use]
    pub fn verified(self) -> bool {
        matches!(self, Self::Exact | Self::Close)
    }
}

/// Where and how well a passage was found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub verdict: Verdict,
    /// Share of the passage's words matched in order (0–1).
    pub score: f64,
    /// Char range in the source text of the best match (empty when missing).
    pub start: usize,
    pub end: usize,
    /// The source's own words over that range: what the evidence actually says.
    pub excerpt: String,
}

/// Find `passage` in `text`.
#[must_use]
pub fn find(passage: &str, text: &str) -> Found {
    let p: Vec<String> = words(passage).into_iter().map(|w| w.norm).collect();
    let t = words(text);
    let missing = Found {
        verdict: Verdict::Missing,
        score: 0.0,
        start: 0,
        end: 0,
        excerpt: String::new(),
    };
    if p.is_empty() || t.is_empty() {
        return missing;
    }
    let found = |score: f64, a: usize, b: usize, verdict: Verdict| {
        let (start, end) = (t[a].start, t[b].end);
        Found {
            verdict,
            score,
            start,
            end,
            excerpt: text.chars().skip(start).take(end - start).collect(),
        }
    };
    let short = p.len() < MIN_WORDS;

    // Exact: the word sequence occurs contiguously.
    if let Some(i) = t
        .windows(p.len())
        .position(|w| w.iter().zip(&p).all(|(a, b)| a.norm == *b))
    {
        let v = if short {
            Verdict::TooShort
        } else {
            Verdict::Exact
        };
        return found(1.0, i, i + p.len() - 1, v);
    }

    // Close: rank windows by bag-of-words overlap (cheap), then align the best few in order.
    let n = p.len();
    let slack = n / 4 + 2;
    let mut bag: std::collections::HashMap<&str, i32> = std::collections::HashMap::new();
    for w in &p {
        *bag.entry(w.as_str()).or_default() += 1;
    }
    let mut candidates: Vec<(usize, usize)> = Vec::new(); // (overlap, window start)
    let mut have: std::collections::HashMap<&str, i32> = std::collections::HashMap::new();
    let mut overlap = 0_usize;
    for (i, w) in t.iter().enumerate() {
        let k = w.norm.as_str();
        if let Some(&need) = bag.get(k) {
            let h = have.entry(k).or_default();
            *h += 1;
            if *h <= need {
                overlap += 1;
            }
        }
        if i >= n {
            let k = t[i - n].norm.as_str();
            if let Some(&need) = bag.get(k) {
                let h = have.entry(k).or_default();
                if *h <= need {
                    overlap -= 1;
                }
                *h -= 1;
            }
        }
        if i + 1 >= n {
            candidates.push((overlap, i + 1 - n));
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut best: Option<(f64, usize, usize)> = None;
    for &(ov, start) in candidates.iter().take(8) {
        #[allow(clippy::cast_precision_loss)]
        if (ov as f64) < PARTIAL * n as f64 {
            break;
        }
        let lo = start.saturating_sub(slack);
        let hi = (start + n + slack).min(t.len());
        let (len, first, last) = lcs(&p, &t[lo..hi]);
        #[allow(clippy::cast_precision_loss)]
        let score = len as f64 / n as f64;
        if best.is_none_or(|b| score > b.0) {
            best = Some((score, lo + first, lo + last));
        }
    }
    match best {
        Some((score, a, b)) if score >= PARTIAL => {
            let v = if short {
                Verdict::TooShort
            } else if score >= CLOSE {
                Verdict::Close
            } else {
                Verdict::Partial
            };
            found(score, a, b, v)
        }
        _ => Found {
            verdict: if short {
                Verdict::TooShort
            } else {
                Verdict::Missing
            },
            ..missing
        },
    }
}

/// Longest common subsequence of `p` in `t`: (length, first and last matched index in `t`).
fn lcs(p: &[String], t: &[Word]) -> (usize, usize, usize) {
    let (n, m) = (p.len(), t.len());
    let mut dp = vec![vec![0_u16; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            dp[i][j] = if p[i - 1] == t[j - 1].norm {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }
    // Walk back to find the matched span.
    let (mut i, mut j) = (n, m);
    let (mut first, mut last) = (usize::MAX, 0);
    while i > 0 && j > 0 {
        if p[i - 1] == t[j - 1].norm {
            first = first.min(j - 1);
            last = last.max(j - 1);
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    (
        usize::from(dp[n][m]),
        if first == usize::MAX { 0 } else { first },
        last,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "Results.\n\nEven the strongest frontier models maintain link validity above 94%,\nyet achieve only 39–77% factual accuracy against their own cited sources. We call this the verification gap.";

    #[test]
    fn exact_ignores_case_punctuation_dashes_and_line_breaks() {
        let f = find(
            "maintain link validity above 94% yet achieve only 39-77% factual accuracy",
            TEXT,
        );
        assert_eq!(f.verdict, Verdict::Exact);
        assert!(
            f.excerpt.starts_with("maintain link validity")
                && f.excerpt.ends_with("factual accuracy"),
            "{}",
            f.excerpt
        );
        assert_eq!(
            TEXT.chars()
                .skip(f.start)
                .take(f.end - f.start)
                .collect::<String>(),
            f.excerpt
        );
    }

    #[test]
    fn an_elision_or_small_slip_is_close() {
        let f = find("strongest frontier models maintain link validity above 94% yet only 39-77% factual accuracy against cited sources", TEXT);
        assert_eq!(f.verdict, Verdict::Close, "{f:?}");
        assert!(f.verdict.verified());
    }

    #[test]
    fn a_paraphrase_or_invention_is_not_verified() {
        let para = find(
            "the best models get links right but are factually accurate less than half the time",
            TEXT,
        );
        assert!(!para.verdict.verified(), "{para:?}");
        let invented = find(
            "frontier models achieve 95% factual accuracy on all benchmarks tested",
            TEXT,
        );
        assert!(!invented.verdict.verified(), "{invented:?}");
    }

    #[test]
    fn reordered_words_do_not_pass() {
        let f = find(
            "sources cited own their against accuracy factual 77 39 only achieve yet",
            TEXT,
        );
        assert!(!f.verdict.verified(), "{f:?}");
    }

    #[test]
    fn short_quotes_are_flagged_even_when_present() {
        assert_eq!(find("verification gap", TEXT).verdict, Verdict::TooShort);
    }

    #[test]
    fn offsets_are_in_chars_not_bytes() {
        let text = "Ünïcödé — prefix. The quick brown fox jumps over the lazy dog today.";
        let f = find("the quick brown fox jumps over the lazy dog", text);
        assert_eq!(f.verdict, Verdict::Exact);
        assert_eq!(f.excerpt, "The quick brown fox jumps over the lazy dog");
    }
}

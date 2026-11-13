//! Analysis of a run: independence groups, corroboration per claim, flags, saturation, and
//! what to do next. Each stage is reported separately so a weak run shows *where* it is weak.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::independence::{groups, shingles, Item, Link};
use crate::ledger::{Run, Stance};
use crate::triage::priority;

/// Something wrong or missing about a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flag {
    /// No verified supporting passage.
    Unverified,
    /// Supported, but all support comes from one independent group.
    SingleVoice,
    /// Central and nobody looked for counter-evidence.
    Unchallenged,
    /// A verified passage argues against it.
    Contested,
}

impl Flag {
    fn label(&self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::SingleVoice => "single voice",
            Self::Unchallenged => "unchallenged",
            Self::Contested => "contested",
        }
    }
}

/// Corroboration of one claim.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimStat {
    pub id: String,
    /// Sources with a verified supporting passage.
    pub support: Vec<String>,
    /// Independent groups among them (each named by its first source).
    pub voices: Vec<String>,
    /// Sources with a verified passage against.
    pub against: Vec<String>,
    /// Citations whose passage was not found.
    pub failed: usize,
    pub flags: Vec<Flag>,
}

/// The whole picture.
#[derive(Debug, Clone)]
pub struct Analysis {
    /// Source id → its group's first source id.
    pub group_of: HashMap<String, String>,
    /// Why sources were joined: (source, source, reason).
    pub joins: Vec<(String, String, String)>,
    pub claims: Vec<ClaimStat>,
    /// Claims added while reading the last `window` sources.
    pub recent_new_claims: usize,
    pub window: usize,
    pub saturated: bool,
}

/// Unread candidates listed in the status report.
pub const UNREAD_SHOWN: usize = 8;

/// Sources read while no new claim appears before a run counts as saturated.
pub const SATURATION_WINDOW: usize = 3;

/// Analyse `run`. `texts` holds the extracted text of every read source, by source id.
#[must_use]
pub fn analyse(run: &Run, texts: &HashMap<String, String>) -> Analysis {
    let read: Vec<_> = run.read().collect();
    let sh: Vec<HashSet<u64>> = read
        .iter()
        .map(|s| texts.get(&s.id).map(|t| shingles(t)).unwrap_or_default())
        .collect();
    // Origins are derived from the url now, not as stored when fetched, so a better rule
    // also corrects older runs.
    let origins: Vec<String> = read
        .iter()
        .map(|s| {
            crate::url::origin(&s.canonical).unwrap_or_else(|| {
                s.fetched
                    .as_ref()
                    .map(|f| f.origin.clone())
                    .unwrap_or_default()
            })
        })
        .collect();
    let items: Vec<Item<'_>> = read
        .iter()
        .zip(&sh)
        .zip(&origins)
        .map(|((s, sh), origin)| {
            let f = s.fetched.as_ref().expect("read sources are fetched");
            Item {
                canonical: &s.canonical,
                origin,
                links: &f.links,
                shingles: sh,
            }
        })
        .collect();
    let (g, j) = groups(&items);
    let group_of: HashMap<String, String> = read
        .iter()
        .zip(&g)
        .map(|(s, &first)| (s.id.clone(), read[first].id.clone()))
        .collect();
    let joins = j
        .into_iter()
        .map(|(a, b, why)| {
            let why = match why {
                Link::SameOrigin => format!("same origin {}", items[a].origin),
                Link::Cites => "one links to the other".to_owned(),
                Link::Copies(c) => format!("{:.0}% shared wording", c * 100.0),
            };
            (read[a].id.clone(), read[b].id.clone(), why)
        })
        .collect();

    let claims = run
        .claims
        .iter()
        .map(|c| {
            let mut support: Vec<String> = Vec::new();
            let mut against: Vec<String> = Vec::new();
            let mut failed = 0;
            for cit in &c.citations {
                if !cit.found.verdict.verified() {
                    failed += 1;
                    continue;
                }
                let list = if cit.stance == Stance::Supports {
                    &mut support
                } else {
                    &mut against
                };
                if !list.contains(&cit.source) {
                    list.push(cit.source.clone());
                }
            }
            let mut voices: Vec<String> = Vec::new();
            for s in &support {
                let g = group_of.get(s).cloned().unwrap_or_else(|| s.clone());
                if !voices.contains(&g) {
                    voices.push(g);
                }
            }
            let mut flags = Vec::new();
            if support.is_empty() {
                flags.push(Flag::Unverified);
            } else if voices.len() < 2 && c.central {
                flags.push(Flag::SingleVoice);
            }
            if c.central && c.challenged_by.is_empty() {
                flags.push(Flag::Unchallenged);
            }
            if !against.is_empty() {
                flags.push(Flag::Contested);
            }
            ClaimStat {
                id: c.id.clone(),
                support,
                voices,
                against,
                failed,
                flags,
            }
        })
        .collect();

    let read_now = read.len();
    let recent_new_claims = run
        .claims
        .iter()
        .filter(|c| c.read_when_added + SATURATION_WINDOW > read_now)
        .count();
    let saturated = read_now >= run.effort.budget().min_sources
        && recent_new_claims == 0
        && !run.claims.is_empty();
    Analysis {
        group_of,
        joins,
        claims,
        recent_new_claims,
        window: SATURATION_WINDOW,
        saturated,
    }
}

/// The status report: budget, claims with corroboration and flags, read order, groups,
/// saturation, next steps.
#[must_use]
pub fn render(run: &Run, a: &Analysis, current_year: i32) -> String {
    let b = run.effort.budget();
    let read = run.read().count();
    let mut md = format!("# {}\n\n", run.question);
    let _ = writeln!(
        md,
        "effort {:?} · searches {}/{} · fetched {}/{} ({read} readable) · claims {} ({} central)\n",
        run.effort,
        run.queries.len(),
        b.searches,
        run.fetch_attempts(),
        b.fetches,
        run.claims.len(),
        run.claims.iter().filter(|c| c.central).count()
    );
    if !run.subquestions.is_empty() {
        md.push_str("## Sub-questions\n");
        for (i, q) in run.subquestions.iter().enumerate() {
            let _ = writeln!(md, "{}. {q}", i + 1);
        }
        md.push('\n');
    }

    md.push_str("## Claims\n");
    if run.claims.is_empty() {
        md.push_str("none yet\n");
    }
    for (c, s) in run.claims.iter().zip(&a.claims) {
        let star = if c.central { "★ " } else { "" };
        let ok = if s.flags.is_empty() { "✓" } else { "·" };
        let _ = writeln!(md, "- {ok} {} {star}{}", c.id, c.text);
        let voices: Vec<String> = s
            .voices
            .iter()
            .map(|g| {
                let members: Vec<&str> = s
                    .support
                    .iter()
                    .filter(|x| a.group_of.get(*x).unwrap_or(x) == g)
                    .map(String::as_str)
                    .collect();
                members.join("+")
            })
            .collect();
        let mut line = format!(
            "  {} independent voice{}",
            s.voices.len(),
            if s.voices.len() == 1 { "" } else { "s" }
        );
        if !voices.is_empty() {
            let _ = write!(line, " ({})", voices.join(" | "));
        }
        if !s.against.is_empty() {
            let _ = write!(line, " · against: {}", s.against.join(", "));
        }
        if s.failed > 0 {
            let _ = write!(
                line,
                " · {} citation{} not found in source",
                s.failed,
                if s.failed == 1 { "" } else { "s" }
            );
        }
        if !c.challenged_by.is_empty() {
            let _ = write!(line, " · challenged by {}", c.challenged_by.join(", "));
        }
        if !s.flags.is_empty() {
            let _ = write!(
                line,
                " · **{}**",
                s.flags
                    .iter()
                    .map(Flag::label)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        md.push_str(&line);
        md.push('\n');
    }

    md.push_str("\n## Sources by reading priority\n");
    let mut order: Vec<(f64, &crate::ledger::SourceRec)> = run
        .sources
        .iter()
        .map(|s| {
            let p =
                s.fetched
                    .as_ref()
                    .map_or(1.0 + s.found_by.len().saturating_sub(1) as f64, |f| {
                        priority(
                            f.triage.class,
                            s.found_by.len(),
                            f.published.as_deref(),
                            current_year,
                        )
                    });
            (p, s)
        })
        .collect();
    order.sort_by(|x, y| y.0.total_cmp(&x.0).then_with(|| x.1.id.cmp(&y.1.id)));
    // Every read source; of the unread candidates only the best few (the list decides what to
    // read next, and a long tail costs tokens without changing that decision).
    let unread_total = order.iter().filter(|(_, s)| s.fetched.is_none()).count();
    let mut unread_shown = 0;
    order.retain(|(_, s)| {
        if s.fetched.is_some() {
            return true;
        }
        unread_shown += 1;
        unread_shown <= UNREAD_SHOWN
    });
    for (p, s) in order {
        let state = match &s.fetched {
            None => "unread".to_owned(),
            Some(f) if !f.ok() => format!("failed: {}", f.error.as_deref().unwrap_or("?")),
            Some(f) => {
                let cites = run
                    .claims
                    .iter()
                    .flat_map(|c| &c.citations)
                    .filter(|c| c.source == s.id && c.found.verdict.verified())
                    .count();
                let group = a
                    .group_of
                    .get(&s.id)
                    .filter(|g| **g != s.id)
                    .map(|g| format!(" · with {g}"))
                    .unwrap_or_default();
                format!(
                    "{:?} {}{}{group} · {cites} verified",
                    f.triage.class,
                    f.published.as_deref().unwrap_or("undated"),
                    if f.triage.reasons.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", f.triage.reasons.join("; "))
                    }
                )
            }
        };
        let _ = writeln!(
            md,
            "- {} [{p:.1}] {} · {} · {state}",
            s.id,
            if s.title.is_empty() {
                &s.canonical
            } else {
                &s.title
            },
            s.canonical
        );
    }

    if unread_total > UNREAD_SHOWN {
        let _ = writeln!(
            md,
            "- … {} more unread candidates",
            unread_total - UNREAD_SHOWN
        );
    }

    if !a.joins.is_empty() {
        md.push_str("\n## Not independent\n");
        for (x, y, why) in &a.joins {
            let _ = writeln!(md, "- {x} ~ {y}: {why}");
        }
    }

    let _ = writeln!(
        md,
        "\n## Saturation\nThe last {} sources read added {} claim{}{}.",
        a.window,
        a.recent_new_claims,
        if a.recent_new_claims == 1 { "" } else { "s" },
        if a.saturated { ": saturated" } else { "" }
    );

    md.push_str("\n## Next\n");
    let next = next_steps(run, a);
    if next.is_empty() {
        md.push_str("- done: every central claim is corroborated and challenged; export it\n");
    }
    for n in next {
        let _ = writeln!(md, "- {n}");
    }
    md
}

/// What to do next, most important first.
#[must_use]
pub fn next_steps(run: &Run, a: &Analysis) -> Vec<String> {
    let b = run.effort.budget();
    let mut out = Vec::new();
    let with = |f: &Flag| {
        a.claims
            .iter()
            .filter(|s| s.flags.contains(f))
            .map(|s| s.id.clone())
            .collect::<Vec<_>>()
    };
    let unverified = with(&Flag::Unverified);
    if !unverified.is_empty() {
        out.push(format!(
            "verify {}: cite a passage that is really in a source, or drop the claim",
            unverified.join(", ")
        ));
    }
    let single = with(&Flag::SingleVoice);
    if !single.is_empty() {
        out.push(format!(
            "find an independent second source for {}",
            single.join(", ")
        ));
    }
    let unchallenged = with(&Flag::Unchallenged);
    if !unchallenged.is_empty() {
        out.push(format!(
            "search for counter-evidence to {} (`search … --challenges C`)",
            unchallenged.join(", ")
        ));
    }
    let unread: Vec<&str> = run
        .sources
        .iter()
        .filter(|s| s.fetched.is_none())
        .map(|s| s.id.as_str())
        .collect();
    if !a.saturated && !unread.is_empty() && run.fetch_attempts() < b.fetches {
        let shown: Vec<&str> = unread.iter().take(8).copied().collect();
        out.push(format!(
            "read more ({} unread: {}{})",
            unread.len(),
            shown.join(" "),
            if unread.len() > 8 { " …" } else { "" }
        ));
    }
    if run.queries.len() >= b.searches || run.fetch_attempts() >= b.fetches {
        out.push(
            "budget reached: export what is supported and list the rest as open questions".into(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Effort, Fetched, Run};
    use crate::triage::{Class, Triage};

    fn read(r: &mut Run, url: &str, origin: &str) -> String {
        let (id, _) = r.add_source(url, url, "", None).unwrap();
        r.source_mut(&id).unwrap().fetched = Some(Fetched {
            at: "t".into(),
            error: None,
            text_file: String::new(),
            chars: 1,
            content_hash: String::new(),
            published: None,
            origin: origin.into(),
            triage: Triage {
                class: Class::Primary,
                reasons: vec![],
            },
            links: vec![],
        });
        id
    }

    #[test]
    fn corroboration_counts_independent_voices_not_pages() {
        let mut r = Run::new("q", Effort::Lookup, "t");
        let text =
            "Only thirty nine to seventy seven percent of cited claims were accurate in the audit.";
        let quote = "thirty nine to seventy seven percent of cited claims were accurate";
        let s1 = read(&mut r, "https://a.com/1", "a.com");
        let s2 = read(&mut r, "https://a.com/2", "a.com");
        let c = r.add_claim("Citations are often inaccurate", true);
        r.cite(&c, &s1, quote, Stance::Supports, text, "t").unwrap();
        r.cite(&c, &s2, quote, Stance::Supports, text, "t").unwrap();
        r.cite(
            &c,
            &s2,
            "invented words that appear nowhere at all in it",
            Stance::Supports,
            text,
            "t",
        )
        .unwrap();
        let texts: HashMap<String, String> =
            [(s1.clone(), text.to_owned()), (s2.clone(), text.to_owned())].into();
        let a = analyse(&r, &texts);
        let st = &a.claims[0];
        assert_eq!(st.support, [s1.clone(), s2.clone()]);
        assert_eq!(st.voices.len(), 1, "same origin: one voice");
        assert_eq!(st.failed, 1);
        assert_eq!(st.flags, [Flag::SingleVoice, Flag::Unchallenged]);

        let s3 = read(&mut r, "https://b.org/x", "b.org");
        r.cite(&c, &s3, quote, Stance::Supports, text, "t").unwrap();
        r.add_query("web", "citations are accurate", Some(&c), "t")
            .unwrap();
        let texts: HashMap<String, String> = [
            (s1, text.to_owned()),
            (
                s2,
                "Another text entirely, nothing in common here at all.".to_owned(),
            ),
            (
                s3,
                "Third source with its own wording about audits of agents.".to_owned(),
            ),
        ]
        .into();
        let a = analyse(&r, &texts);
        assert_eq!(a.claims[0].voices.len(), 2);
        assert!(a.claims[0].flags.is_empty(), "{:?}", a.claims[0].flags);
        let md = render(&r, &a, 2026);
        assert!(md.contains("✓ C1 ★ Citations are often inaccurate"), "{md}");
        assert!(md.contains("S1 ~ S2: same origin a.com"), "{md}");
    }

    #[test]
    fn saturation_needs_enough_reading_and_no_new_claims() {
        let mut r = Run::new("q", Effort::Lookup, "t");
        r.add_claim("early claim", false);
        for i in 0..3 {
            read(&mut r, &format!("https://s{i}.org"), &format!("s{i}.org"));
        }
        let a = analyse(&r, &HashMap::new());
        assert!(
            a.saturated,
            "3 read since the last new claim, min 3 for lookup"
        );
        r.add_claim("late claim", false);
        assert!(!analyse(&r, &HashMap::new()).saturated);
    }

    /// Regression: the first real run listed all 44 unread candidates in every status report.
    #[test]
    fn status_lists_read_sources_and_only_the_best_unread() {
        let mut r = Run::new("q", Effort::Lookup, "t");
        read(&mut r, "https://read.org/a", "read.org");
        for i in 0..(UNREAD_SHOWN + 4) {
            r.add_source(&format!("https://u{i}.org"), "", "", None)
                .unwrap();
        }
        let md = render(&r, &analyse(&r, &HashMap::new()), 2026);
        assert!(md.contains("https://read.org/a"));
        assert_eq!(md.matches("· unread").count(), UNREAD_SHOWN, "{md}");
        assert!(md.contains("… 4 more unread candidates"), "{md}");
    }
}

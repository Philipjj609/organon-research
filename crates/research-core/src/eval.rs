//! Evaluation: score a run stage by stage against a case, so a weak run shows *where* it is
//! weak (plan, retrieval, source quality, claim–passage alignment, synthesis) and what it cost.
//!
//! Scoring is deterministic: term matching against the ledger, never a model's opinion. A stage
//! whose gold is missing (no expected aspects, sources or key facts) scores `None`, not zero:
//! unknown stays unknown.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use organon_protocol::{Cost, Episode, EpisodeId, EpisodeType, Outcome};
use serde::{Deserialize, Serialize};

use crate::analysis::{Analysis, Flag};
use crate::ledger::{Effort, Run, Stance};
use crate::matching::words;
use crate::triage::Class;
use crate::url;
use crate::Error;

/// Format version of an evaluation set.
pub const SET_VERSION: u32 = 1;

/// The stages, in pipeline order.
pub const STAGES: [&str; 5] = [
    "plan",
    "retrieval",
    "source_quality",
    "alignment",
    "synthesis",
];

/// A set of questions with what a good run should cover.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalSet {
    pub version: u32,
    pub cases: Vec<Case>,
}

impl EvalSet {
    /// Parse and check a set.
    ///
    /// # Errors
    /// Bad JSON, an unknown version, duplicate case ids, or an expectation without terms.
    pub fn parse(json: &str) -> Result<Self, Error> {
        let set: Self = serde_json::from_str(json)?;
        if set.version != SET_VERSION {
            return Err(Error::Invalid(format!(
                "eval set version {} (this tool reads {SET_VERSION})",
                set.version
            )));
        }
        let mut ids = HashSet::new();
        for c in &set.cases {
            if !ids.insert(c.id.as_str()) {
                return Err(Error::Invalid(format!("duplicate case id {}", c.id)));
            }
            for e in c.aspects.iter().chain(&c.key_facts) {
                if e.terms.is_empty() || e.terms.iter().any(|t| t.trim().is_empty()) {
                    return Err(Error::Invalid(format!(
                        "case {}: '{}' needs non-empty terms",
                        c.id, e.text
                    )));
                }
            }
            for s in &c.expected_sources {
                if url::canonical(&s.url).is_none() {
                    return Err(Error::Invalid(format!(
                        "case {}: expected source is not an http(s) URL: {}",
                        c.id, s.url
                    )));
                }
            }
        }
        Ok(set)
    }
}

/// One question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub question: String,
    /// Where the question comes from (a run, a memory claim, a conversation).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provenance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// The run to score: a folder holding `run.json`, or a slug. None: not run yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Plan: parts of the question the sub-questions and queries should cover.
    #[serde(default)]
    pub aspects: Vec<Expect>,
    /// Retrieval: sources a good run should find.
    #[serde(default)]
    pub expected_sources: Vec<ExpectedSource>,
    /// Synthesis: facts a good run should state in a verified claim.
    #[serde(default)]
    pub key_facts: Vec<Expect>,
}

/// Something the run's text should contain. Each term must occur (as a word sequence, any
/// case and punctuation); `a|b` accepts either alternative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expect {
    pub text: String,
    pub terms: Vec<String>,
    /// Where this expectation comes from (a decision, a saved source).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub basis: String,
}

impl Expect {
    /// Every term (one of its alternatives) occurs in `text`.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        let hay: Vec<String> = words(text).into_iter().map(|w| w.norm).collect();
        self.terms.iter().all(|t| {
            t.split('|').any(|alt| {
                let needle: Vec<String> = words(alt).into_iter().map(|w| w.norm).collect();
                !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle.as_slice())
            })
        })
    }
}

/// A source a good run should find.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedSource {
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// One stage's result. `score` is None when the stage has no gold or nothing to measure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub score: Option<f64>,
    pub detail: String,
}

impl Stage {
    fn of(hit: usize, of: usize, detail: String) -> Self {
        Self {
            score: (of > 0).then(|| ratio(hit, of)),
            detail,
        }
    }
}

/// What the run spent, from the ledger (plus tokens/money when measured elsewhere).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Spend {
    pub searches: usize,
    pub search_budget: usize,
    pub sources_found: usize,
    pub fetch_attempts: usize,
    pub fetched: usize,
    pub fetch_budget: usize,
    pub claims: usize,
    pub citations: usize,
    pub verified_citations: usize,
    #[serde(default)]
    pub cost: Measured,
}

/// Cost measured outside the ledger (the reader's tokens, time, money). Fields are optional:
/// what was not measured stays unknown.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Measured {
    #[serde(flatten)]
    pub cost: Cost,
    /// Total tokens when only a total is known (e.g. a subagent's usage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
}

/// A case's scores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseScore {
    pub id: String,
    pub question: String,
    /// The run scored (None: not run).
    pub run: Option<String>,
    pub effort: Option<Effort>,
    /// Stage name → result, in [`STAGES`] order.
    pub stages: BTreeMap<String, Stage>,
    pub spend: Spend,
    /// Claim flags across the run (unverified, single voice, unchallenged, contested).
    pub flags: BTreeMap<String, usize>,
}

impl CaseScore {
    /// A case with no run: every stage unknown.
    #[must_use]
    pub fn not_run(case: &Case) -> Self {
        Self {
            id: case.id.clone(),
            question: case.question.clone(),
            run: None,
            effort: case.effort,
            stages: STAGES
                .iter()
                .map(|s| {
                    (
                        (*s).to_owned(),
                        Stage {
                            score: None,
                            detail: "not run".into(),
                        },
                    )
                })
                .collect(),
            spend: Spend::default(),
            flags: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn stage(&self, name: &str) -> Option<f64> {
        self.stages.get(name).and_then(|s| s.score)
    }
}

/// How much a cited source's kind is worth as evidence (primary 1 … derivative 0).
#[must_use]
pub fn evidence_weight(class: Class) -> f64 {
    match class {
        Class::Primary => 1.0,
        Class::Reference => 0.75,
        Class::Practitioner => 0.5,
        Class::Discussion | Class::Unknown => 0.25,
        Class::Derivative => 0.0,
    }
}

/// Score `run` against `case`. `a` is the run's analysis (for flags).
#[must_use]
pub fn score(case: &Case, run: &Run, a: &Analysis, cost: Measured) -> CaseScore {
    let mut stages = BTreeMap::new();

    // Plan: aspects of the question covered by the sub-questions and queries.
    let plan_text: String = run
        .subquestions
        .iter()
        .chain(run.queries.iter().map(|q| &q.text))
        .map(|s| format!("{s}\n"))
        .collect();
    let missed: Vec<&str> = case
        .aspects
        .iter()
        .filter(|e| !e.matches(&plan_text))
        .map(|e| e.text.as_str())
        .collect();
    let hit = case.aspects.len() - missed.len();
    stages.insert(
        "plan".into(),
        Stage::of(
            hit,
            case.aspects.len(),
            format!(
                "{hit}/{} aspects · {} sub-questions · {} queries{}",
                case.aspects.len(),
                run.subquestions.len(),
                run.queries.len(),
                missing(&missed)
            ),
        ),
    );

    // Retrieval: expected sources found (a page under an expected URL counts), read, cited.
    let verified_support: HashSet<&str> = run
        .claims
        .iter()
        .flat_map(|c| &c.citations)
        .filter(|ci| ci.stance == Stance::Supports && ci.found.verdict.verified())
        .map(|ci| ci.source.as_str())
        .collect();
    let (mut found, mut read, mut cited) = (0, 0, 0);
    let mut not_found = Vec::new();
    for e in &case.expected_sources {
        let want = url::canonical(&e.url).unwrap_or_default();
        let hits: Vec<_> = run
            .sources
            .iter()
            .filter(|s| s.canonical == want || s.canonical.starts_with(&format!("{want}/")))
            .collect();
        if hits.is_empty() {
            not_found.push(e.url.as_str());
            continue;
        }
        found += 1;
        if hits
            .iter()
            .any(|s| s.fetched.as_ref().is_some_and(|f| f.ok()))
        {
            read += 1;
        }
        if hits
            .iter()
            .any(|s| verified_support.contains(s.id.as_str()))
        {
            cited += 1;
        }
    }
    let n = case.expected_sources.len();
    stages.insert(
        "retrieval".into(),
        Stage::of(
            found,
            n,
            format!(
                "{found}/{n} found · {read} read · {cited} cited{}",
                missing(&not_found)
            ),
        ),
    );

    // Source quality: what the verified evidence rests on.
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut weight = 0.0;
    let mut cited_n = 0;
    for s in &run.sources {
        if !verified_support.contains(s.id.as_str()) {
            continue;
        }
        let class = s
            .fetched
            .as_ref()
            .map_or(Class::Unknown, |f| f.triage.class);
        weight += evidence_weight(class);
        cited_n += 1;
        *classes
            .entry(format!("{class:?}").to_lowercase())
            .or_default() += 1;
    }
    stages.insert(
        "source_quality".into(),
        Stage {
            score: (cited_n > 0).then(|| weight / to_f64(cited_n)),
            detail: if cited_n == 0 {
                "no verified evidence".into()
            } else {
                format!(
                    "{cited_n} cited sources: {}",
                    classes
                        .iter()
                        .map(|(k, v)| format!("{v} {k}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        },
    );

    // Alignment: claims carrying a passage that is really there (containment, not entailment).
    let citations: Vec<_> = run.claims.iter().flat_map(|c| &c.citations).collect();
    let verified = citations
        .iter()
        .filter(|c| c.found.verdict.verified())
        .count();
    let supported = run
        .claims
        .iter()
        .filter(|c| {
            c.citations
                .iter()
                .any(|ci| ci.stance == Stance::Supports && ci.found.verdict.verified())
        })
        .count();
    stages.insert(
        "alignment".into(),
        Stage::of(
            supported,
            run.claims.len(),
            format!(
                "{supported}/{} claims with a verified passage · quotes {verified}/{} verified",
                run.claims.len(),
                citations.len()
            ),
        ),
    );

    // Synthesis: key facts stated by a verified claim.
    let supported_text: String = run
        .claims
        .iter()
        .filter(|c| {
            c.citations
                .iter()
                .any(|ci| ci.stance == Stance::Supports && ci.found.verdict.verified())
        })
        .map(|c| format!("{}\n", c.text))
        .collect();
    let missed: Vec<&str> = case
        .key_facts
        .iter()
        .filter(|e| !e.matches(&supported_text))
        .map(|e| e.text.as_str())
        .collect();
    let hit = case.key_facts.len() - missed.len();
    stages.insert(
        "synthesis".into(),
        Stage::of(
            hit,
            case.key_facts.len(),
            format!(
                "{hit}/{} key facts{}",
                case.key_facts.len(),
                missing(&missed)
            ),
        ),
    );

    let mut flags = BTreeMap::new();
    for c in &a.claims {
        for f in &c.flags {
            let k = match f {
                Flag::Unverified => "unverified",
                Flag::SingleVoice => "single_voice",
                Flag::Unchallenged => "unchallenged",
                Flag::Contested => "contested",
            };
            *flags.entry(k.to_owned()).or_default() += 1;
        }
    }

    let budget = run.effort.budget();
    CaseScore {
        id: case.id.clone(),
        question: case.question.clone(),
        run: Some(run.slug.clone()),
        effort: Some(run.effort),
        stages,
        spend: Spend {
            searches: run.queries.len(),
            search_budget: budget.searches,
            sources_found: run.sources.len(),
            fetch_attempts: run.fetch_attempts(),
            fetched: run.read().count(),
            fetch_budget: budget.fetches,
            claims: run.claims.len(),
            citations: citations.len(),
            verified_citations: verified,
            cost,
        },
        flags,
    }
}

/// Mean of one stage over the cases that have a score, and how many do.
#[must_use]
pub fn mean(scores: &[CaseScore], stage: &str) -> (Option<f64>, usize) {
    let v: Vec<f64> = scores.iter().filter_map(|s| s.stage(stage)).collect();
    if v.is_empty() {
        (None, 0)
    } else {
        (Some(v.iter().sum::<f64>() / to_f64(v.len())), v.len())
    }
}

/// The results as a table: one row per case, then the mean per stage.
#[must_use]
pub fn render(scores: &[CaseScore]) -> String {
    let mut out = String::from(
        "| case | effort | plan | retrieval | sources | alignment | synthesis | searches | read | quotes ok |\n\
         |---|---|---|---|---|---|---|---|---|---|\n",
    );
    for s in scores {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.id,
            s.effort
                .map_or("—".into(), |e| format!("{e:?}").to_lowercase()),
            cell(s.stage("plan")),
            cell(s.stage("retrieval")),
            cell(s.stage("source_quality")),
            cell(s.stage("alignment")),
            cell(s.stage("synthesis")),
            if s.run.is_some() {
                s.spend.searches.to_string()
            } else {
                "not run".into()
            },
            s.spend.fetched,
            if s.spend.citations > 0 {
                format!("{}/{}", s.spend.verified_citations, s.spend.citations)
            } else {
                "—".into()
            },
        );
    }
    let _ = write!(out, "| **mean** | |");
    for st in STAGES {
        let (m, n) = mean(scores, st);
        let _ = write!(out, " {} (n={n}) |", cell(m));
    }
    out.push_str(" | | |\n");
    out
}

/// A research episode carrying the case's scores and spend, for organon-learning.
#[must_use]
pub fn episode(s: &CaseScore, started: &str, ended: &str) -> Episode {
    let task = format!("eval:{}", s.id);
    Episode {
        id: EpisodeId::of(format!("research-eval\n{task}\n{started}").as_bytes()),
        kind: EpisodeType::Research,
        task,
        started: started.to_owned(),
        ended: Some(ended.to_owned()),
        inputs: vec![
            serde_json::json!({ "question": s.question, "run": s.run, "effort": s.effort }),
        ],
        outputs: vec![serde_json::json!({
            "stages": s.stages.iter().map(|(k, v)| (k.clone(), v.score)).collect::<BTreeMap<_, _>>(),
            "spend": { "searches": s.spend.searches, "fetched": s.spend.fetched,
                       "claims": s.spend.claims, "citations": s.spend.citations,
                       "verified_citations": s.spend.verified_citations },
            "flags": s.flags,
            "tokens": s.spend.cost.tokens,
        })],
        outcome: Outcome::Unknown,
        cost: s.spend.cost.cost.clone(),
        trace: None,
    }
}

/// When the run started and when it last did something (RFC 3339 strings sort by time).
#[must_use]
pub fn span(run: &Run) -> (String, String) {
    let last = run
        .queries
        .iter()
        .map(|q| q.at.as_str())
        .chain(
            run.sources
                .iter()
                .filter_map(|s| s.fetched.as_ref().map(|f| f.at.as_str())),
        )
        .chain(
            run.claims
                .iter()
                .flat_map(|c| &c.citations)
                .map(|c| c.at.as_str()),
        )
        .max()
        .unwrap_or(&run.created)
        .max(run.created.as_str());
    (run.created.clone(), last.to_owned())
}

fn cell(v: Option<f64>) -> String {
    v.map_or("—".into(), |x| format!("{x:.2}"))
}

fn missing(items: &[&str]) -> String {
    if items.is_empty() {
        String::new()
    } else {
        format!(" · missing: {}", items.join("; "))
    }
}

fn ratio(a: usize, b: usize) -> f64 {
    to_f64(a) / to_f64(b)
}

#[allow(clippy::cast_precision_loss)]
fn to_f64(n: usize) -> f64 {
    n as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expect(text: &str, terms: &[&str]) -> Expect {
        Expect {
            text: text.into(),
            terms: terms.iter().map(|t| (*t).to_owned()).collect(),
            basis: String::new(),
        }
    }

    #[test]
    fn terms_match_word_sequences_with_alternatives() {
        let e = expect(
            "tesseract weak on code",
            &["tesseract", "57%|57 percent", "code"],
        );
        assert!(e.matches("Tesseract reads only 57% of CODE tokens."));
        assert!(!e.matches("Tesseract reads 75% of code"));
        // Words, not substrings: "code" does not match inside "decoder".
        assert!(!expect("x", &["code"]).matches("a decoder"));
        assert!(expect("x", &["open source|open-source"]).matches("an open-source engine"));
    }

    #[test]
    fn sets_are_checked() {
        assert!(EvalSet::parse(r#"{"version":2,"cases":[]}"#).is_err());
        let dup = r#"{"version":1,"cases":[{"id":"a","question":"q"},{"id":"a","question":"r"}]}"#;
        assert!(EvalSet::parse(dup).is_err());
        let empty_terms = r#"{"version":1,"cases":[{"id":"a","question":"q","aspects":[{"text":"t","terms":[]}]}]}"#;
        assert!(EvalSet::parse(empty_terms).is_err());
        let bad_url = r#"{"version":1,"cases":[{"id":"a","question":"q","expected_sources":[{"url":"nope"}]}]}"#;
        assert!(EvalSet::parse(bad_url).is_err());
    }

    #[test]
    fn a_case_without_a_run_is_unknown_not_zero() {
        let case: Case = serde_json::from_str(r#"{"id":"a","question":"q"}"#).unwrap();
        let s = CaseScore::not_run(&case);
        assert!(STAGES.iter().all(|st| s.stage(st).is_none()));
        assert_eq!(mean(&[s], "plan"), (None, 0));
    }
}

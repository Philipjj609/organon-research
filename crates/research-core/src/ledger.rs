//! The ledger: everything a research run did, as data (`run.json`), so a run is resumable,
//! auditable, and scored stage by stage. Short local ids (`Q3`, `S12`, `C4`) keep it typeable.

use serde::{Deserialize, Serialize};

use crate::matching::{find, Found};
use crate::triage::Triage;
use crate::url;
use crate::Error;

/// How hard to look: sets the budget (Anthropic's published effort-scaling rules as a start).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    /// A fact: a few searches, a handful of sources.
    Lookup,
    /// Options side by side.
    Compare,
    /// A question with many parts.
    Deep,
}

impl std::str::FromStr for Effort {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "lookup" => Ok(Self::Lookup),
            "compare" => Ok(Self::Compare),
            "deep" => Ok(Self::Deep),
            other => Err(Error::Unknown(format!(
                "effort '{other}' (lookup, compare, deep)"
            ))),
        }
    }
}

/// Limits for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub searches: usize,
    pub fetches: usize,
    /// Do not call a run saturated before reading this many sources.
    pub min_sources: usize,
}

impl Effort {
    #[must_use]
    pub fn budget(self) -> Budget {
        match self {
            Self::Lookup => Budget {
                searches: 10,
                fetches: 8,
                min_sources: 3,
            },
            Self::Compare => Budget {
                searches: 30,
                fetches: 25,
                min_sources: 8,
            },
            Self::Deep => Budget {
                searches: 80,
                fetches: 60,
                min_sources: 15,
            },
        }
    }
}

/// A search that was run (by this tool, or by the agent's own web search: engine `web`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub id: String,
    pub engine: String,
    pub text: String,
    /// The claim this query challenges, when it is a search for counter-evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenges: Option<String>,
    pub at: String,
    pub results: usize,
}

/// A candidate or read source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRec {
    pub id: String,
    pub url: String,
    pub canonical: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub snippet: String,
    /// Queries that surfaced it (a source found from several angles is worth more).
    pub found_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched: Option<Fetched>,
}

/// What fetching a source gave.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fetched {
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Path of the extracted text, relative to the run folder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text_file: String,
    pub chars: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    pub origin: String,
    pub triage: Triage,
    /// Canonical outbound links (for citation-based independence).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
}

impl Fetched {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

/// Whether a citation backs or undercuts its claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Supports,
    Against,
}

/// A quoted passage offered as evidence, with what verification found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub source: String,
    /// The passage as the reader quoted it.
    pub passage: String,
    pub stance: Stance,
    pub found: Found,
    pub at: String,
}

/// A claim the research supports (or fails to).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimRec {
    pub id: String,
    pub text: String,
    /// Central claims must be corroborated and challenged before the run is done.
    pub central: bool,
    /// Sources read when the claim was added (saturation: are new sources still adding claims?).
    pub read_when_added: usize,
    pub citations: Vec<Citation>,
    /// Query ids that searched for counter-evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub challenged_by: Vec<String>,
}

/// One research run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub question: String,
    pub slug: String,
    pub effort: Effort,
    pub created: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subquestions: Vec<String>,
    #[serde(default)]
    pub queries: Vec<Query>,
    #[serde(default)]
    pub sources: Vec<SourceRec>,
    #[serde(default)]
    pub claims: Vec<ClaimRec>,
}

impl Run {
    #[must_use]
    pub fn new(question: &str, effort: Effort, now: &str) -> Self {
        Self {
            question: question.trim().to_owned(),
            slug: slug(question),
            effort,
            created: now.to_owned(),
            subquestions: Vec::new(),
            queries: Vec::new(),
            sources: Vec::new(),
            claims: Vec::new(),
        }
    }

    /// Sources fetched successfully: the ones that can be cited.
    pub fn read(&self) -> impl Iterator<Item = &SourceRec> {
        self.sources
            .iter()
            .filter(|s| s.fetched.as_ref().is_some_and(Fetched::ok))
    }

    #[must_use]
    pub fn fetch_attempts(&self) -> usize {
        self.sources.iter().filter(|s| s.fetched.is_some()).count()
    }

    pub fn add_query(
        &mut self,
        engine: &str,
        text: &str,
        challenges: Option<&str>,
        now: &str,
    ) -> Result<String, Error> {
        if let Some(c) = challenges {
            self.claim(c)?;
        }
        let id = format!("Q{}", self.queries.len() + 1);
        self.queries.push(Query {
            id: id.clone(),
            engine: engine.to_owned(),
            text: text.trim().to_owned(),
            challenges: challenges.map(str::to_owned),
            at: now.to_owned(),
            results: 0,
        });
        if let Some(c) = challenges {
            self.claim_mut(c)?.challenged_by.push(id.clone());
        }
        Ok(id)
    }

    /// Add a candidate, or note that another query found it again. Returns its id and whether
    /// it is new.
    ///
    /// # Errors
    /// Not an absolute http(s) URL.
    pub fn add_source(
        &mut self,
        raw_url: &str,
        title: &str,
        snippet: &str,
        query: Option<&str>,
    ) -> Result<(String, bool), Error> {
        let canonical = url::canonical(raw_url)
            .ok_or_else(|| Error::Invalid(format!("not an http(s) URL: {raw_url}")))?;
        if let Some(q) = query {
            if let Some(qr) = self.queries.iter_mut().find(|x| x.id == q) {
                qr.results += 1;
            }
        }
        if let Some(s) = self.sources.iter_mut().find(|s| s.canonical == canonical) {
            if let Some(q) = query {
                if !s.found_by.iter().any(|x| x == q) {
                    s.found_by.push(q.to_owned());
                }
            }
            if s.title.is_empty() {
                title.clone_into(&mut s.title);
            }
            return Ok((s.id.clone(), false));
        }
        let id = format!("S{}", self.sources.len() + 1);
        self.sources.push(SourceRec {
            id: id.clone(),
            url: raw_url.trim().to_owned(),
            canonical,
            title: title.trim().to_owned(),
            snippet: snippet.trim().to_owned(),
            found_by: query.map(|q| vec![q.to_owned()]).unwrap_or_default(),
            fetched: None,
        });
        Ok((id, true))
    }

    pub fn source(&self, id: &str) -> Result<&SourceRec, Error> {
        self.sources
            .iter()
            .find(|s| s.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| Error::Unknown(format!("source {id}")))
    }

    pub fn source_mut(&mut self, id: &str) -> Result<&mut SourceRec, Error> {
        self.sources
            .iter_mut()
            .find(|s| s.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| Error::Unknown(format!("source {id}")))
    }

    pub fn claim(&self, id: &str) -> Result<&ClaimRec, Error> {
        self.claims
            .iter()
            .find(|c| c.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| Error::Unknown(format!("claim {id}")))
    }

    fn claim_mut(&mut self, id: &str) -> Result<&mut ClaimRec, Error> {
        self.claims
            .iter_mut()
            .find(|c| c.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| Error::Unknown(format!("claim {id}")))
    }

    pub fn add_claim(&mut self, text: &str, central: bool) -> String {
        let id = format!("C{}", self.claims.len() + 1);
        let read_when_added = self.read().count();
        self.claims.push(ClaimRec {
            id: id.clone(),
            text: text.trim().to_owned(),
            central,
            read_when_added,
            citations: Vec::new(),
            challenged_by: Vec::new(),
        });
        id
    }

    /// Verify `passage` against the source's text and record the citation, whatever the
    /// verdict (failed citations are kept: they measure how often the reader misquotes).
    ///
    /// # Errors
    /// Unknown claim or source, or the source was not read.
    pub fn cite(
        &mut self,
        claim: &str,
        source: &str,
        passage: &str,
        stance: Stance,
        source_text: &str,
        now: &str,
    ) -> Result<&Citation, Error> {
        let s = self.source(source)?;
        if !s.fetched.as_ref().is_some_and(Fetched::ok) {
            return Err(Error::Invalid(format!(
                "{} has not been read (fetch it first)",
                s.id
            )));
        }
        let sid = s.id.clone();
        let found = find(passage, source_text);
        let c = self.claim_mut(claim)?;
        c.citations.push(Citation {
            source: sid,
            passage: passage.trim().to_owned(),
            stance,
            found,
            at: now.to_owned(),
        });
        Ok(c.citations.last().expect("just pushed"))
    }
}

/// A folder-friendly name for a question.
#[must_use]
pub fn slug(question: &str) -> String {
    let words: Vec<String> = question
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .filter(|w| {
            !matches!(
                w.as_str(),
                "the"
                    | "a"
                    | "an"
                    | "of"
                    | "to"
                    | "is"
                    | "are"
                    | "what"
                    | "how"
                    | "do"
                    | "does"
                    | "for"
                    | "in"
                    | "and"
                    | "which"
            )
        })
        .take(6)
        .collect();
    if words.is_empty() {
        "question".into()
    } else {
        words.join("-")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triage::{Class, Triage};

    pub(crate) fn fetched(origin: &str) -> Fetched {
        Fetched {
            at: "t".into(),
            error: None,
            text_file: String::new(),
            chars: 100,
            content_hash: "h".into(),
            published: None,
            origin: origin.into(),
            triage: Triage {
                class: Class::Primary,
                reasons: vec![],
            },
            links: vec![],
        }
    }

    #[test]
    fn the_same_page_found_twice_is_one_source_found_from_two_angles() {
        let mut r = Run::new(
            "How do research agents verify citations?",
            Effort::Compare,
            "t",
        );
        assert_eq!(r.slug, "research-agents-verify-citations");
        let q1 = r
            .add_query("openalex", "citation verification", None, "t")
            .unwrap();
        let q2 = r
            .add_query("web", "research agent hallucinated citations", None, "t")
            .unwrap();
        let (a, new_a) = r
            .add_source(
                "https://arxiv.org/pdf/2605.06635v2",
                "Cited but not verified",
                "",
                Some(&q1),
            )
            .unwrap();
        let (b, new_b) = r
            .add_source("https://arxiv.org/abs/2605.06635", "", "", Some(&q2))
            .unwrap();
        assert_eq!(
            (a.as_str(), new_a, b.as_str(), new_b),
            ("S1", true, "S1", false)
        );
        assert_eq!(r.sources[0].found_by, [q1, q2]);
        assert!(r.add_source("ftp://x", "", "", None).is_err());
    }

    #[test]
    fn citations_are_verified_and_kept_whatever_the_verdict() {
        let mut r = Run::new("q", Effort::Lookup, "t");
        r.add_source("https://a.org/p", "A", "", None).unwrap();
        let c = r.add_claim("Agents misquote sources", true);
        let text = "Even the strongest frontier models achieve only 39 to 77 percent factual accuracy against their own cited sources.";
        assert!(
            r.cite(
                &c,
                "S1",
                "achieve only 39 to 77 percent factual accuracy",
                Stance::Supports,
                text,
                "t"
            )
            .is_err(),
            "unread source"
        );
        r.source_mut("S1").unwrap().fetched = Some(fetched("a.org"));
        let ok = r
            .cite(
                &c,
                "s1",
                "achieve only 39 to 77 percent factual accuracy",
                Stance::Supports,
                text,
                "t",
            )
            .unwrap();
        assert!(ok.found.verdict.verified());
        let bad = r
            .cite(
                &c,
                "S1",
                "models are accurate nearly all the time in our tests",
                Stance::Supports,
                text,
                "t",
            )
            .unwrap();
        assert!(!bad.found.verdict.verified());
        assert_eq!(r.claims[0].citations.len(), 2);
    }

    #[test]
    fn challenges_attach_to_their_claim() {
        let mut r = Run::new("q", Effort::Lookup, "t");
        let c = r.add_claim("X works", true);
        let q = r.add_query("web", "X fails", Some(&c), "t").unwrap();
        assert_eq!(r.claims[0].challenged_by, [q]);
        assert!(r.add_query("web", "Y", Some("C9"), "t").is_err());
    }
}

//! organon-research-core: the mechanics of research with receipts, as pure logic.
//!
//! - [`ledger`]: a run as data: queries, sources, claims, citations (resumable, auditable).
//! - [`matching`]: does a quoted passage really occur in the source, and where?
//! - [`triage`]: what kind of voice a source is, with reasons.
//! - [`independence`]: which sources are one voice (same origin, citation, copied text).
//! - [`analysis`]: corroboration per claim, flags, saturation, next steps, the status report.
//! - [`export`]: protocol messages and a proposal for memory.
//! - [`eval`]: score a run stage by stage against an evaluation case.
//! - [`search`], [`text`], [`url`]: backend parsers, HTML to text, canonical URLs and origins.
//!
//! No network, no files, no printing: the CLI fetches and stores, the reader (a model or a
//! person) reads and proposes claims, and this crate keeps both honest.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod eval;
pub mod export;
pub mod independence;
pub mod ledger;
pub mod matching;
pub mod search;
pub mod text;
pub mod triage;
pub mod url;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown {0}")]
    Unknown(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

//! Export a run to memory: organon-protocol messages (`mneme ingest`) and a proposal whose
//! claims cite the verified passages by observation id (`mneme digest`).
//!
//! Only verified passages travel. A claim with none is left out of the proposal and listed as
//! an open question instead, so memory never receives a citation nobody could find.

use std::collections::HashMap;

use organon_protocol::{
    Location, Message, Observation, ObservationKind, ProducedBy, Source, SourceId,
};
use serde_json::{json, Value as J};

use crate::analysis::{Analysis, Flag};
use crate::ledger::{Run, Stance};
use crate::triage::Class;

/// The protocol source id of a page: its canonical URL is its identity.
#[must_use]
pub fn source_id(canonical: &str) -> SourceId {
    SourceId::of(format!("web:{canonical}").as_bytes())
}

/// What an export produces.
#[derive(Debug)]
pub struct Export {
    pub messages: Vec<Message>,
    pub proposal: J,
    /// Claims left out for lack of a verified passage.
    pub dropped: Vec<String>,
}

/// Build the export. `about` is the memory anchor the findings are about (it must exist in
/// memory or be added by the proposal's reader); `run_dir` is recorded so text files can be found.
#[must_use]
pub fn export(
    run: &Run,
    a: &Analysis,
    about: &str,
    about_name: Option<&str>,
    produced_by: &ProducedBy,
    run_dir: &str,
) -> Export {
    let stats: HashMap<&str, _> = a.claims.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut messages = Vec::new();
    let mut passage_ids: HashMap<(String, usize, usize), String> = HashMap::new();

    // Sources that back at least one verified citation, then their passages.
    let cited: Vec<&str> = run
        .claims
        .iter()
        .flat_map(|c| &c.citations)
        .filter(|c| c.found.verdict.verified())
        .map(|c| c.source.as_str())
        .collect();
    for s in run.read().filter(|s| cited.contains(&s.id.as_str())) {
        let f = s.fetched.as_ref().expect("read");
        let sid = source_id(&s.canonical);
        let kind = if f.triage.class == Class::Primary && s.canonical.contains("arxiv.org") {
            "paper"
        } else {
            "web-page"
        };
        let mut meta = serde_json::Map::new();
        meta.insert("origin".into(), J::from(f.origin.clone()));
        meta.insert(
            "class".into(),
            serde_json::to_value(f.triage.class).unwrap_or(J::Null),
        );
        if let Some(p) = &f.published {
            meta.insert("published".into(), J::from(p.clone()));
        }
        meta.insert("research".into(), J::from(run.slug.clone()));
        messages.push(Message::Source(Source {
            id: sid.clone(),
            kind: kind.into(),
            uri: s.url.clone(),
            title: s.title.clone(),
            captured_at: f.at.clone(),
            content_hash: f.content_hash.clone(),
            files: [("text".to_owned(), format!("{run_dir}/{}", f.text_file))]
                .into_iter()
                .collect(),
            meta: meta.into_iter().collect(),
            produced_by: Some(produced_by.clone()),
        }));
        for cit in run
            .claims
            .iter()
            .flat_map(|c| &c.citations)
            .filter(|c| c.source == s.id && c.found.verdict.verified())
        {
            let key = (s.id.clone(), cit.found.start, cit.found.end);
            if passage_ids.contains_key(&key) {
                continue;
            }
            let o = Observation::new(
                sid.clone(),
                ObservationKind::new(ObservationKind::PASSAGE),
                Location::Text {
                    start: cit.found.start as u64,
                    end: cit.found.end as u64,
                },
                json!({ "text": cit.found.excerpt }),
                produced_by.clone(),
            );
            passage_ids.insert(key, o.id.to_string());
            messages.push(Message::Observation(o));
        }
    }

    let mut claims = Vec::new();
    let mut dropped = Vec::new();
    for c in &run.claims {
        let st = stats[c.id.as_str()];
        let evidence = |stance: Stance| -> Vec<String> {
            c.citations
                .iter()
                .filter(|x| x.stance == stance && x.found.verdict.verified())
                .filter_map(|x| {
                    passage_ids
                        .get(&(x.source.clone(), x.found.start, x.found.end))
                        .cloned()
                })
                .collect()
        };
        let support = evidence(Stance::Supports);
        if support.is_empty() {
            dropped.push(c.id.clone());
            claims.push(json!({ "about": about, "relation": "question", "value": format!("Unverified: {}", c.text) }));
            continue;
        }
        let mut why = vec![format!(
            "{} independent voice{} ({})",
            st.voices.len(),
            if st.voices.len() == 1 { "" } else { "s" },
            st.support
                .iter()
                .filter_map(|s| run.source(s).ok())
                .map(|s| s
                    .fetched
                    .as_ref()
                    .map_or(s.canonical.clone(), |f| f.origin.clone()))
                .collect::<Vec<_>>()
                .join(", ")
        )];
        if st.flags.contains(&Flag::Unchallenged) {
            why.push("not challenged".into());
        }
        if st.flags.contains(&Flag::Contested) {
            why.push(format!("contested by {}", st.against.join(", ")));
        }
        claims.push(json!({
            "about": about,
            "relation": "claims",
            "value": c.text,
            "evidence": support,
            "rationale": format!("research '{}': {}", run.slug, why.join("; ")),
        }));
        let against = evidence(Stance::Against);
        if !against.is_empty() {
            // The protocol has no negated stance: a disagreement stays an open question that
            // cites the opposing passages.
            claims.push(json!({ "about": about, "relation": "question", "value": format!("Contested: {}", c.text), "evidence": against,
                                "rationale": format!("research '{}': verified counter-evidence", run.slug) }));
        }
    }
    for q in &run.subquestions {
        if run.claims.is_empty() {
            claims.push(json!({ "about": about, "relation": "question", "value": q }));
        }
    }
    let mut proposal = json!({
        "author": format!("{}@{}", produced_by.organ, produced_by.version),
        "message": format!("research: {}", run.question),
        "basis": "stated",
        "claims": claims,
    });
    if let Some(name) = about_name {
        // A new topic: the proposal introduces its anchor so digest accepts the claims.
        proposal["anchors"] = json!([{ "id": about, "name": name, "scope": "world" }]);
    }
    Export {
        messages,
        proposal,
        dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyse;
    use crate::ledger::{Effort, Fetched};
    use crate::triage::Triage;

    #[test]
    fn verified_passages_become_observations_cited_by_id_unverified_claims_become_questions() {
        let mut r = Run::new("q", Effort::Lookup, "t");
        let (s, _) = r
            .add_source("https://a.org/p", "A paper", "", None)
            .unwrap();
        r.source_mut(&s).unwrap().fetched = Some(Fetched {
            at: "2026-11-13T00:00:00Z".into(),
            error: None,
            text_file: "sources/S1.txt".into(),
            chars: 90,
            content_hash: "abc".into(),
            published: Some("2026-05-07".into()),
            origin: "a.org".into(),
            triage: Triage {
                class: Class::Primary,
                reasons: vec![],
            },
            links: vec![],
        });
        let text = "Link validity stays above ninety four percent while factual accuracy ranges from thirty nine to seventy seven percent.";
        let good = r.add_claim("Valid links do not mean accurate citations", true);
        let bad = r.add_claim("Accuracy is perfect", false);
        r.cite(
            &good,
            &s,
            "link validity stays above ninety four percent while factual accuracy ranges",
            Stance::Supports,
            text,
            "t",
        )
        .unwrap();
        r.cite(
            &bad,
            &s,
            "accuracy is perfect in every single case we measured",
            Stance::Supports,
            text,
            "t",
        )
        .unwrap();
        let a = analyse(&r, &[(s.clone(), text.to_owned())].into());
        let pb = ProducedBy {
            organ: "organon-research".into(),
            version: "0.1.0".into(),
            tool: None,
            model: None,
        };
        let e = export(&r, &a, "concept:x", Some("X"), &pb, "runs/q");
        assert_eq!(e.proposal["anchors"][0]["id"], "concept:x");
        assert_eq!(e.dropped, ["C2"]);
        let Message::Observation(o) = &e.messages[1] else {
            panic!("{:?}", e.messages)
        };
        assert_eq!(o.at, Location::Text { start: 0, end: 75 });
        assert_eq!(
            o.value["text"],
            "Link validity stays above ninety four percent while factual accuracy ranges"
        );
        let claims = e.proposal["claims"].as_array().unwrap();
        assert_eq!(claims[0]["evidence"][0], o.id.to_string());
        assert!(claims[0]["rationale"]
            .as_str()
            .unwrap()
            .contains("1 independent voice (a.org)"));
        assert_eq!(claims[1]["relation"], "question");
        let Message::Source(src) = &e.messages[0] else {
            panic!()
        };
        assert_eq!(src.files["text"], "runs/q/sources/S1.txt");
    }
}

//! Evaluation end to end: a run built through the real ledger (with a fake fetcher standing in
//! for HTTP), analysed, then scored stage by stage against a case.

use std::collections::HashMap;

use organon_research_core::analysis::analyse;
use organon_research_core::eval::{self, CaseScore, EvalSet, Measured};
use organon_research_core::ledger::{Effort, Fetched, Run, Stance};
use organon_research_core::triage::classify;

const NOW: &str = "2026-11-16T10:00:00Z";

/// Stands in for the network: url → page text.
struct FakeFetcher(HashMap<&'static str, &'static str>);

impl FakeFetcher {
    fn new() -> Self {
        Self(HashMap::from([
            (
                "https://arxiv.org/abs/2401.00001",
                "We benchmark OCR engines on code in screencasts. Tesseract recognised only 57% \
                 of code tokens correctly, while a vision-language model reached 76% on the same \
                 frames. Errors cluster on long identifiers and brackets.",
            ),
            (
                "https://github.com/acme/fast-ocr",
                "fast-ocr is a pure Rust OCR engine. It currently recognizes the Latin alphabet \
                 only and is an early preview with more errors than commercial engines.",
            ),
            (
                "https://top10.example.com/best-ocr-2026",
                "Top 10 best OCR tools in 2026! Number one will surprise you.",
            ),
        ]))
    }

    /// Fetch `id`'s url into the run the way the CLI does: text, origin, triage.
    fn fetch(&self, run: &mut Run, id: &str, texts: &mut HashMap<String, String>) {
        let s = run.source_mut(id).unwrap();
        let text = self.0[s.canonical.as_str()];
        s.fetched = Some(Fetched {
            at: NOW.into(),
            error: None,
            text_file: format!("sources/{id}.txt"),
            chars: text.chars().count(),
            content_hash: "h".into(),
            published: None,
            origin: s.origin(),
            triage: classify(&s.canonical, &s.title, &[]),
            links: vec![],
        });
        texts.insert(id.to_owned(), text.to_owned());
    }
}

fn run_and_texts() -> (Run, HashMap<String, String>) {
    let web = FakeFetcher::new();
    let mut run = Run::new("Which OCR reads code best?", Effort::Lookup, NOW);
    run.subquestions
        .push("How accurate is Tesseract on code?".into());
    let q = run
        .add_query("arxiv", "ocr screencast code", None, NOW)
        .unwrap();
    let (paper, _) = run
        .add_source(
            "https://arxiv.org/pdf/2401.00001v2",
            "OCR on code",
            "",
            Some(&q),
        )
        .unwrap();
    let (repo, _) = run
        .add_source(
            "https://github.com/acme/fast-ocr",
            "acme/fast-ocr",
            "",
            Some(&q),
        )
        .unwrap();
    let (list, _) = run
        .add_source(
            "https://top10.example.com/best-ocr-2026",
            "Top 10 best OCR tools 2026",
            "",
            Some(&q),
        )
        .unwrap();
    let mut texts = HashMap::new();
    for id in [&paper, &repo, &list] {
        web.fetch(&mut run, id, &mut texts);
    }

    // C1: verified from the paper. C2: verified from the repo. C3: misquoted only.
    let c1 = run.add_claim(
        "Tesseract reads only 57% of code tokens; a VLM reaches 76%.",
        true,
    );
    let t = texts[&paper].clone();
    run.cite(
        &c1,
        &paper,
        "Tesseract recognised only 57% of code tokens correctly",
        Stance::Supports,
        &t,
        NOW,
    )
    .unwrap();
    let c2 = run.add_claim("fast-ocr reads Latin script only.", false);
    let t = texts[&repo].clone();
    run.cite(
        &c2,
        &repo,
        "It currently recognizes the Latin alphabet only",
        Stance::Supports,
        &t,
        NOW,
    )
    .unwrap();
    let c3 = run.add_claim("The best OCR tool is a surprise.", false);
    let t = texts[&list].clone();
    run.cite(
        &c3,
        &list,
        "the number one OCR tool is a total surprise to everyone",
        Stance::Supports,
        &t,
        NOW,
    )
    .unwrap();
    (run, texts)
}

const SET: &str = r#"{
  "version": 1,
  "cases": [
    {
      "id": "ocr",
      "question": "Which OCR reads code best?",
      "aspects": [
        {"text": "Tesseract accuracy", "terms": ["tesseract"]},
        {"text": "Rust engines", "terms": ["rust"]}
      ],
      "expected_sources": [
        {"url": "https://arxiv.org/abs/2401.00001"},
        {"url": "https://github.com/acme/fast-ocr"},
        {"url": "https://example.org/never-found"}
      ],
      "key_facts": [
        {"text": "Tesseract 57% on code", "terms": ["tesseract", "57%"]},
        {"text": "VLM beats OCR", "terms": ["vlm|vision-language", "76%"]},
        {"text": "surprise winner", "terms": ["surprise"]}
      ]
    },
    {"id": "later", "question": "Not run yet"}
  ]
}"#;

fn scored() -> CaseScore {
    let set = EvalSet::parse(SET).unwrap();
    let (run, texts) = run_and_texts();
    let a = analyse(&run, &texts);
    eval::score(&set.cases[0], &run, &a, Measured::default())
}

#[test]
fn each_stage_is_scored_on_its_own() {
    let s = scored();
    // Plan: "tesseract" is in a sub-question, "rust" nowhere in sub-questions or queries.
    assert_eq!(s.stage("plan"), Some(0.5));
    // Retrieval: the arXiv PDF v2 counts as the abstract page; the repo is found; one is not.
    assert!((s.stage("retrieval").unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert!(s.stages["retrieval"].detail.contains("never-found"));
    // Source quality: both cited sources are primary (paper host, code host); the listicle's
    // quote failed, so it is not evidence.
    assert_eq!(s.stage("source_quality"), Some(1.0));
    // Alignment: 2 of 3 claims carry a passage that is really there.
    assert!((s.stage("alignment").unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert!(s.stages["alignment"].detail.contains("quotes 2/3 verified"));
    // Synthesis: facts count only from verified claims, so "surprise" (C3, misquoted) misses.
    assert!((s.stage("synthesis").unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert!(s.stages["synthesis"].detail.contains("surprise winner"));
    assert_eq!(s.spend.fetched, 3);
    assert_eq!(s.spend.search_budget, 10);
    assert_eq!(s.flags.get("unverified"), Some(&1));
}

#[test]
fn missing_gold_is_unknown_and_left_out_of_the_mean() {
    let mut set = EvalSet::parse(SET).unwrap();
    set.cases[0].key_facts.clear();
    let (run, texts) = run_and_texts();
    let a = analyse(&run, &texts);
    let s = eval::score(&set.cases[0], &run, &a, Measured::default());
    assert_eq!(s.stage("synthesis"), None);
    let all = vec![s, CaseScore::not_run(&set.cases[1])];
    assert_eq!(eval::mean(&all, "synthesis"), (None, 0));
    assert_eq!(eval::mean(&all, "plan"), (Some(0.5), 1));
    let table = eval::render(&all);
    assert!(table.contains("| later | — | — |"));
    assert!(table.contains("0.50 (n=1)"));
}

#[test]
fn episodes_carry_stage_scores_and_measured_cost() {
    let mut s = scored();
    s.spend.cost = serde_json::from_str(r#"{"tokens": 99070, "seconds": 94.0}"#).unwrap();
    let (run, _) = run_and_texts();
    let (started, ended) = eval::span(&run);
    let e = eval::episode(&s, &started, &ended);
    let v = serde_json::to_value(organon_protocol::Message::Episode(e)).unwrap();
    assert_eq!(v["type"], "episode");
    assert_eq!(v["kind"], "research");
    assert_eq!(v["task"], "eval:ocr");
    assert_eq!(v["cost"]["seconds"], 94.0);
    assert_eq!(v["outputs"][0]["tokens"], 99070);
    assert_eq!(v["outputs"][0]["stages"]["plan"], 0.5);
    assert!(v["outputs"][0]["stages"]["synthesis"].as_f64().is_some());
}

#[test]
fn the_documented_example_set_parses() {
    let set = EvalSet::parse(include_str!("../../../eval/example.json")).unwrap();
    assert_eq!(set.cases.len(), 2);
    assert!(set.cases.iter().all(|c| c.run.is_none()));
}

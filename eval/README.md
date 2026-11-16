# Evaluating research runs

`organon-research eval <set.json>` scores runs against a set of questions, **stage by stage**. A weak run should show *where* it is weak: in the plan, in what it found, in what its evidence rests on, in its quotes, or in what it concluded. It also shows what each run spent.

```bash
organon-research eval set.json                         # table per case, mean per stage
organon-research eval set.json --costs costs.json \
    --out results.json --episodes ~/.organon/episodes.jsonl
```

Keep a set of real questions, with its expected sources and facts, wherever the questions belong. If they are personal, that means a private memory, not this repo. [example.json](example.json) shows the format.

## Set format (version 1)

```json
{
  "version": 1,
  "cases": [{
    "id": "ocr-code",
    "question": "…",
    "provenance": "where the question comes from",
    "effort": "lookup | compare | deep",
    "run": "<run folder (relative to the set) or slug>; omit if not run yet",
    "aspects":          [{ "text": "…", "terms": ["a", "b|c"], "basis": "…" }],
    "expected_sources": [{ "url": "https://…", "why": "…" }],
    "key_facts":        [{ "text": "…", "terms": ["…"], "basis": "where this fact comes from" }]
  }]
}
```

How **terms** match:
- Every term must occur in the text as a sequence of words. Case and punctuation are ignored, and words are matched whole, so "code" does not match inside "decoder".
- `a|b` accepts either alternative.
- Keep terms few and specific: a name, a number, a distinguishing word.

`--costs` maps each case id to whatever was measured outside the ledger, for example `{"q07": {"tokens": 99070, "seconds": 94, "model": "claude-opus-5-5", "usd": 0.6}}`. Leave out what you did not measure.

## Rubric

Each stage scores between 0 and 1. A stage **without gold scores unknown (—), not 0**, and is left out of the mean (`n` says how many cases count). Scoring is deterministic: the ledger is matched against terms, and no model gives its opinion.

| stage | score | gold needed | detail shown |
|---|---|---|---|
| **plan** | share of `aspects` whose terms occur in the sub-questions or queries | aspects | sub-question and query counts, missed aspects |
| **retrieval** | share of `expected_sources` found. A source counts if its canonical URL equals the expected one or lies under it, and arXiv PDF and version links fold into the abstract page. | expected sources | how many were read, how many were cited with a verified passage, which were missed |
| **source quality** | mean evidence weight of the sources cited with a verified supporting passage: primary 1, reference 0.75, practitioner 0.5, discussion and unknown 0.25, derivative 0 | none | class mix |
| **alignment** | share of claims that have at least one verified supporting passage | none | quotes verified out of quotes attempted |
| **synthesis** | share of `key_facts` stated by a claim that has a verified supporting passage | key facts | missed facts |

Spend comes from the ledger: searches against their budget, sources found, fetches attempted and succeeded against their budget, claims, and citations. Measured cost (`--costs`) is added on top. The claim flags (unverified, single voice, unchallenged, contested) are counted too.

### What the scores do not say

- **Alignment is containment, not entailment.** A verified passage is really in the source, but it may still not *support* the claim. Treat alignment as an upper bound until a verifier model judges entailment.
- **Term matching is literal.** A key fact stated in other words is missed. Write terms for the distinguishing word or number, and read the missed facts before trusting a low synthesis score.
- **Sources are matched by URL.** Another copy of the same work (a DOI instead of the arXiv page, the paper's code repository) counts as missed. Read the missed sources before trusting a low retrieval score.
- **Gold comes from people.** Expected sources and key facts should come from what the asker saved, named, or decided, never from an earlier run's own output, which would make the score circular. Where there is no such gold, leave it out, and the stage stays unknown.

## Episodes

`--episodes <file>` appends one organon-protocol `episode` message per scored case to the file: kind `research`, task `eval:<id>`, outputs holding the stage scores, spend and flags, and cost taken from `--costs`. organon-learning can read them next to task episodes.

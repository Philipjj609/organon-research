---
name: research
description: Research a question with receipts using organon-research. Every claim must cite a verbatim passage that the tool finds in the fetched source; independent voices are counted; central claims are challenged; the result goes to memory. Use for any question that needs sources, not for things you already know.
---

# Research with receipts

You read and judge. The tool keeps the ledger, fetches, triages, **verifies every quote against the source text**, groups sources into independent voices, and tells you what is still missing. `R=organon-research`, `X=<run slug or folder>`.

## The loop

1. **Frame.** `R new "<question>" --effort lookup|compare|deep`. Write 2–4 sub-questions (`R sub X "…" "…"`), including what memory does not know yet (`mneme recall --brief`, `mneme frontier`).
2. **Search wide, then narrow.** Start broad: `R search X all <short query>`. Then go specific: `R search X openalex|arxiv|hn|github <query>`. Use your own web search too, and add what it finds: `R add X <url>… --query "<what you searched>"`. Use several phrasings ("X problems", "X vs Y", "X benchmark").
3. **Read the best first.** `R fetch X` reads the next 5 by priority (primary > practitioner > reference > discussion > listicles). `R fetch X S12 S7` reads chosen ones. Paywalled DOI? Search for an open copy (author page, arXiv, NSF/PMC) and add it. `R text X S12` gives the text file; read it with your file tools (grep first, then the relevant parts).
4. **Claim with passages.** `R claim X "<one checkable statement>" [--central]`, then `R cite X C3 S12 "<verbatim passage, ≥6 words>"`. Copy the passage character for character from the text file. **NOT verified** means you misquoted or paraphrased: fix the quote, never the check. A passage that argues against a claim: `--against`.
5. **Challenge every central claim.** `R search X <engine> "<the claim's weak point>" --challenges C3`, or `R add … --challenges C3`. Look for its strongest counter-evidence (newer versions, other domains, vendor bias) and cite what you find with `--against`. A disagreement stays a disagreement.
6. **Check.** `R status X` (also written to `status.md`) shows, per claim, its independent voices, failed quotes, challenges and flags (`unverified`, `single voice`, `unchallenged`, `contested`), plus saturation and next steps. Repeat steps 2–5 until central claims have 2+ voices and are challenged, the run saturates, or the budget runs out.
7. **Write back.** `R export X --about concept:… [--name "…" if new]`, read `proposal.json`, then `mneme ingest messages.jsonl` and `mneme digest proposal.json`. Unverified claims become open questions automatically. Put your own synthesis (recommendation, what to do) in a separate proposal with `basis: inferred` and a confidence, citing the `o_…` passages.

## Rules

- No claim without a verified passage. No passage you did not read in the fetched text.
- Count voices, not pages: the same company, author, or copied text is one voice (the tool groups them; the report says why).
- Prefer primary sources (papers, docs, code, data). A blog or listicle is a lead to a primary source, not evidence.
- Keep claims small and checkable: one number, one comparison, one mechanism.
- State what is missing. A single-voice or contested claim is still useful, as long as it is labelled.
- Budget is real: lookup ≈ 10 searches / 8 sources, compare ≈ 30 / 25, deep ≈ 80 / 60.

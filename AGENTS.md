# Working in organon-research (for AI agents)

**Using it** (researching a question): follow [skills/research/SKILL.md](skills/research/SKILL.md).

**Changing it:** [organon/docs/ENGINEERING.md](https://github.com/Philipjj609/organon/blob/main/docs/ENGINEERING.md) first, then [docs/DESIGN.md](docs/DESIGN.md).

**Layout:**
- `crates/research-core` (`organon-research-core`): pure logic, with no network, no files and no printing.
  - `ledger`: the run as data.
  - `matching`: passage verification.
  - `triage`, `independence`, `analysis`, `export`.
  - `search`: backend parsers.
  - `text`, `url`.
- `crates/research-cli` (`organon-research`): the binary.
  - `net`: the only HTTP code.
  - `main`: commands, files, pdftotext.

**Quality gate** (CI on Linux, macOS, Windows): `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test`. Every bug fix gets a regression test.

**Rules:**
1. Verification is deterministic and strict. Never loosen a threshold to make a quote pass; fix the quote.
2. Failed citations and failed fetches are kept in the ledger. They are measurements.
3. Triage gives reasons, not truth. Independence counts are an upper bound.
4. Run folders live in `$ORGANON_HOME/research`, never in a repo (they hold third-party text).

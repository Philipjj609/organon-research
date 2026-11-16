//! `organon-research`: research with receipts.
//!
//! A run lives in a folder (default `$ORGANON_HOME/research/<slug>`, `ORGANON_HOME` = `~/.organon`):
//! `run.json` (the ledger), `sources/S3.txt` (extracted text of each read source), `status.md`,
//! and after export `messages.jsonl` + `proposal.json` for memory.
//!
//! The reader (a model or a person) frames, searches, reads and proposes claims with quoted
//! passages; this tool keeps the ledger, fetches and triages, verifies every quote against the
//! source text, groups sources into independent voices, and says what is still missing.

#![forbid(unsafe_code)]

mod net;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _};
use clap::{Parser, Subcommand};
use organon_protocol::{sha256_hex, ProducedBy};
use organon_research_core::analysis::{analyse, next_steps, render};
use organon_research_core::export::export;
use organon_research_core::ledger::{Effort, Fetched, Run, Stance};
use organon_research_core::matching::Verdict;
use organon_research_core::search::Engine;
use organon_research_core::triage::{classify, priority, Class, Triage};
use organon_research_core::{text, url};

const ORGAN: &str = "organon-research";
/// Readable text shorter than this is treated as a failed read (script-rendered page, paywall).
const MIN_TEXT_CHARS: usize = 300;

#[derive(Parser)]
#[command(
    name = "organon-research",
    version,
    about = "Research with receipts: every claim tied to a passage that is really there"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a run
    New {
        question: String,
        /// lookup | compare | deep (sets the budget)
        #[arg(long, default_value = "compare")]
        effort: String,
        /// Run folder (default: $ORGANON_HOME/research/<slug>)
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Add sub-questions (the plan)
    Sub { run: String, questions: Vec<String> },
    /// Search a keyless backend (openalex, arxiv, hn, github, or all) and add the hits
    Search {
        run: String,
        engine: String,
        query: Vec<String>,
        #[arg(long, default_value_t = 8)]
        n: usize,
        /// This search looks for counter-evidence to a claim
        #[arg(long)]
        challenges: Option<String>,
    },
    /// Add sources found elsewhere (e.g. your own web search), optionally with the query used
    Add {
        run: String,
        urls: Vec<String>,
        /// The web query that found them (recorded as a search)
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        challenges: Option<String>,
    },
    /// Fetch and read sources: given ids, or the next N unread by priority
    Fetch {
        run: String,
        ids: Vec<String>,
        #[arg(long, default_value_t = 5)]
        next: usize,
    },
    /// Where a read source's text is (read it with your own tools)
    Text { run: String, source: String },
    /// Add a claim the research should support
    Claim {
        run: String,
        text: String,
        /// Central claims must be corroborated and challenged
        #[arg(long)]
        central: bool,
    },
    /// Cite a verbatim passage of a read source for a claim; the passage is verified
    Cite {
        run: String,
        claim: String,
        source: String,
        passage: String,
        /// The passage argues against the claim
        #[arg(long)]
        against: bool,
    },
    /// Corroboration, flags, reading order, saturation, next steps (also written to status.md)
    Status { run: String },
    /// Write messages.jsonl (`mneme ingest`) and proposal.json (`mneme digest`)
    Export {
        run: String,
        /// Memory anchor the findings are about (e.g. concept:dependency-inversion)
        #[arg(long)]
        about: String,
        /// Name for the anchor when it is new to memory (the proposal then introduces it)
        #[arg(long)]
        name: Option<String>,
    },
    /// Score runs against an evaluation set, stage by stage (plan, retrieval, source quality,
    /// alignment, synthesis), with what each run spent
    Eval {
        /// The evaluation set (JSON; see eval/README.md)
        set: PathBuf,
        /// Cost measured outside the ledger, per case id: {"q07": {"tokens": 99070, "seconds": 94}}
        #[arg(long)]
        costs: Option<PathBuf>,
        /// Write the scores as JSON
        #[arg(long)]
        out: Option<PathBuf>,
        /// Append one research episode per scored case (organon-protocol JSONL)
        #[arg(long)]
        episodes: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::New {
            question,
            effort,
            dir,
        } => new(&question, effort.parse()?, dir),
        Cmd::Sub { run, questions } => with_run(&run, |r, _| {
            r.subquestions.extend(
                questions
                    .iter()
                    .map(|q| q.trim().to_owned())
                    .filter(|q| !q.is_empty()),
            );
            for (i, q) in r.subquestions.iter().enumerate() {
                println!("{}. {q}", i + 1);
            }
            Ok(())
        }),
        Cmd::Search {
            run,
            engine,
            query,
            n,
            challenges,
        } => search(&run, &engine, &query.join(" "), n, challenges.as_deref()),
        Cmd::Add {
            run,
            urls,
            query,
            challenges,
        } => add(&run, &urls, query.as_deref(), challenges.as_deref()),
        Cmd::Fetch { run, ids, next } => fetch(&run, &ids, next),
        Cmd::Text { run, source } => {
            let (dir, r) = load(&run)?;
            let s = r.source(&source)?;
            match &s.fetched {
                Some(f) if f.ok() => println!(
                    "{}  ({} chars) {}",
                    dir.join(&f.text_file).display(),
                    f.chars,
                    s.title
                ),
                _ => bail!("{} has not been read", s.id),
            }
            Ok(())
        }
        Cmd::Claim { run, text, central } => with_run(&run, |r, _| {
            println!("{}", r.add_claim(&text, central));
            Ok(())
        }),
        Cmd::Cite {
            run,
            claim,
            source,
            passage,
            against,
        } => cite(&run, &claim, &source, &passage, against),
        Cmd::Status { run } => status(&run),
        Cmd::Export { run, about, name } => export_run(&run, &about, name.as_deref()),
        Cmd::Eval {
            set,
            costs,
            out,
            episodes,
        } => eval_set(&set, costs.as_deref(), out.as_deref(), episodes.as_deref()),
    }
}

// ---- run folders -------------------------------------------------------------------------

fn organon_home() -> PathBuf {
    std::env::var_os("ORGANON_HOME").map_or_else(
        || {
            PathBuf::from(
                std::env::var_os("USERPROFILE")
                    .or_else(|| std::env::var_os("HOME"))
                    .unwrap_or_default(),
            )
            .join(".organon")
        },
        PathBuf::from,
    )
}

/// A run argument is a folder holding `run.json`, or a slug under `$ORGANON_HOME/research`.
fn run_dir(arg: &str) -> PathBuf {
    let p = Path::new(arg);
    if p.join("run.json").exists() {
        p.to_path_buf()
    } else {
        organon_home().join("research").join(arg)
    }
}

fn load(arg: &str) -> anyhow::Result<(PathBuf, Run)> {
    let dir = run_dir(arg);
    let path = dir.join("run.json");
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "no run at {} (start one with `organon-research new`)",
            dir.display()
        )
    })?;
    Ok((
        dir,
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
    ))
}

fn save(dir: &Path, run: &Run) -> anyhow::Result<()> {
    let tmp = dir.join("run.json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(run)?)?;
    fs::rename(&tmp, dir.join("run.json"))?;
    Ok(())
}

/// Load, change, save.
fn with_run(
    arg: &str,
    f: impl FnOnce(&mut Run, &Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let (dir, mut run) = load(arg)?;
    let result = f(&mut run, &dir);
    save(&dir, &run)?;
    result
}

fn new(question: &str, effort: Effort, dir: Option<PathBuf>) -> anyhow::Result<()> {
    let run = Run::new(question, effort, &clock::now());
    let dir = dir.unwrap_or_else(|| organon_home().join("research").join(&run.slug));
    if dir.join("run.json").exists() {
        bail!(
            "a run already exists at {} (use it, or pass --dir)",
            dir.display()
        );
    }
    fs::create_dir_all(dir.join("sources"))?;
    save(&dir, &run)?;
    let b = effort.budget();
    println!("{}", dir.display());
    println!(
        "run '{}' · effort {effort:?}: up to {} searches, {} sources; saturation after ≥{} sources",
        run.slug, b.searches, b.fetches, b.min_sources
    );
    Ok(())
}

// ---- searching -----------------------------------------------------------------------------

fn search(
    arg: &str,
    engine: &str,
    query: &str,
    n: usize,
    challenges: Option<&str>,
) -> anyhow::Result<()> {
    if query.trim().is_empty() {
        bail!("empty query");
    }
    let engines: Vec<Engine> = if engine == "all" {
        Engine::ALL.to_vec()
    } else {
        vec![engine.parse()?]
    };
    let mut http = net::Http::new();
    with_run(arg, |r, _| {
        for e in engines {
            let q = r.add_query(e.name(), query, challenges, &clock::now())?;
            let req = e.url(query, n);
            let hits = match http
                .get(
                    &req,
                    "application/json, application/atom+xml",
                    net::gap_for(&req),
                )
                .and_then(|b| Ok(e.parse(&b.text())?))
            {
                Ok(h) => h,
                Err(err) => {
                    println!("{q} {}: failed ({err:#})", e.name());
                    continue;
                }
            };
            println!("{q} {} \"{query}\": {} hits", e.name(), hits.len());
            for h in hits {
                match r.add_source(&h.url, &h.title, &h.snippet, Some(&q)) {
                    Ok((id, is_new)) => println!(
                        "  {id}{} {} · {}",
                        if is_new { "" } else { " (again)" },
                        h.title,
                        h.snippet.chars().take(110).collect::<String>()
                    ),
                    Err(err) => println!("  skipped: {err}"),
                }
            }
        }
        Ok(())
    })
}

fn add(
    arg: &str,
    urls: &[String],
    query: Option<&str>,
    challenges: Option<&str>,
) -> anyhow::Result<()> {
    with_run(arg, |r, _| {
        let q = query
            .map(|q| r.add_query("web", q, challenges, &clock::now()))
            .transpose()?;
        for u in urls {
            let (id, is_new) = r.add_source(u, "", "", q.as_deref())?;
            println!(
                "{id}{} {}",
                if is_new { "" } else { " (already known)" },
                r.source(&id)?.canonical
            );
        }
        Ok(())
    })
}

// ---- reading -------------------------------------------------------------------------------

fn fetch(arg: &str, ids: &[String], next: usize) -> anyhow::Result<()> {
    let year = clock::year();
    let mut http = net::Http::new();
    with_run(arg, |r, dir| {
        let budget = r.effort.budget().fetches;
        let chosen: Vec<String> = if ids.is_empty() {
            let mut unread: Vec<(f64, String)> = r
                .sources
                .iter()
                .filter(|s| s.fetched.is_none())
                .map(|s| {
                    // Before reading, only the URL and title are known.
                    let t = classify(&s.canonical, &s.title, &[]);
                    (
                        priority(t.class, s.found_by.len(), None, year),
                        s.id.clone(),
                    )
                })
                .collect();
            unread.sort_by(|a, b| b.0.total_cmp(&a.0));
            unread.into_iter().take(next).map(|(_, id)| id).collect()
        } else {
            ids.to_vec()
        };
        if r.fetch_attempts() + chosen.len() > budget {
            println!("note: this goes past the budget of {budget} sources");
        }
        for id in chosen {
            let s = r.source(&id)?.clone();
            let mut title = s.title.clone();
            let f = match read_source(&mut http, dir, &s.id, &s.url, &s.canonical) {
                Ok((mut f, page_title)) => {
                    if title.is_empty() {
                        title = page_title;
                    }
                    f.triage = classify(&s.canonical, &title, &f.links);
                    f
                }
                Err(e) => Fetched {
                    at: clock::now(),
                    error: Some(format!("{e:#}")),
                    text_file: String::new(),
                    chars: 0,
                    content_hash: String::new(),
                    published: None,
                    origin: url::origin(&s.canonical).unwrap_or_default(),
                    triage: classify(&s.canonical, &s.title, &[]),
                    links: Vec::new(),
                },
            };
            let src = r.source_mut(&id)?;
            src.title = title;
            match &f.error {
                None => println!(
                    "{} read: {:?} · {} chars · {}",
                    src.id, f.triage.class, f.chars, src.canonical
                ),
                Some(e) => println!("{} failed: {e} · {}", src.id, src.canonical),
            }
            src.fetched = Some(f);
        }
        Ok(())
    })
}

/// Fetch one source, extract its text, store it. Returns the record (triage still to be done
/// by the caller, which knows the best title) and the page's own title.
fn read_source(
    http: &mut net::Http,
    dir: &Path,
    id: &str,
    raw: &str,
    canonical: &str,
) -> anyhow::Result<(Fetched, String)> {
    // arXiv: the full-text HTML version when it exists, else the abstract page.
    let candidates: Vec<String> = match url::arxiv_id(canonical) {
        Some(aid) => vec![
            format!("https://arxiv.org/html/{aid}"),
            canonical.to_owned(),
        ],
        None => vec![raw.to_owned()],
    };
    let mut last_err = None;
    for u in &candidates {
        match http.get(u, "text/html, text/plain;q=0.9, */*;q=0.1", net::gap_for(u)) {
            Ok(body) => {
                let e = if body.content_type.contains("pdf") || body.bytes.starts_with(b"%PDF") {
                    match pdf_text(&body.bytes, dir, id) {
                        Ok(t) => text::Extracted {
                            // A PDF's first line is almost always its title.
                            title: t
                                .lines()
                                .map(str::trim)
                                .find(|l| !l.is_empty())
                                .unwrap_or_default()
                                .chars()
                                .take(200)
                                .collect(),
                            text: t,
                            ..Default::default()
                        },
                        Err(err) => {
                            last_err = Some(err);
                            continue;
                        }
                    }
                } else {
                    let page = body.text();
                    if body.content_type.contains("html") || page.trim_start().starts_with('<') {
                        text::extract(&page, u)
                    } else {
                        text::Extracted {
                            text: page,
                            ..Default::default()
                        }
                    }
                };
                let chars = e.text.chars().count();
                if chars < MIN_TEXT_CHARS {
                    last_err = Some(anyhow::anyhow!(
                        "only {chars} characters of text (script-rendered page or paywall?)"
                    ));
                    continue;
                }
                let file = format!("sources/{id}.txt");
                fs::create_dir_all(dir.join("sources"))?;
                fs::write(dir.join(&file), &e.text)?;
                let links: Vec<String> = e
                    .links
                    .iter()
                    .filter_map(|l| url::canonical(l))
                    .filter(|l| l != canonical)
                    .collect();
                let fetched = Fetched {
                    at: clock::now(),
                    error: None,
                    text_file: file,
                    chars,
                    content_hash: sha256_hex(e.text.as_bytes()),
                    published: e.published,
                    origin: url::origin(canonical).unwrap_or_default(),
                    triage: Triage {
                        class: Class::Unknown,
                        reasons: Vec::new(),
                    },
                    links,
                };
                return Ok((fetched, e.title));
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("nothing to fetch")))
}

/// Text of a PDF via poppler's `pdftotext`; words hyphenated across lines are rejoined so
/// quotes match. The PDF is kept next to its text (`sources/S3.pdf`).
fn pdf_text(bytes: &[u8], dir: &Path, id: &str) -> anyhow::Result<String> {
    let pdf = dir.join("sources").join(format!("{id}.pdf"));
    let out = dir.join("sources").join(format!("{id}.pdf.txt"));
    fs::create_dir_all(dir.join("sources"))?;
    fs::write(&pdf, bytes)?;
    let status = std::process::Command::new("pdftotext")
        .args(["-enc", "UTF-8"])
        .arg(&pdf)
        .arg(&out)
        .status()
        .context("PDF: needs `pdftotext` (poppler-utils) on PATH")?;
    if !status.success() {
        bail!("pdftotext failed on {}", pdf.display());
    }
    let raw = fs::read_to_string(&out)?;
    fs::remove_file(&out).ok();
    Ok(dehyphenate(&raw.replace('\u{c}', "\n")))
}

/// Join `exam-\nple` into `example` (a letter, a hyphen, a line break, a lowercase letter).
fn dehyphenate(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '-'
            && i > 0
            && chars[i - 1].is_alphabetic()
            && chars.get(i + 1) == Some(&'\n')
            && chars.get(i + 2).is_some_and(|c| c.is_lowercase())
        {
            i += 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn texts(dir: &Path, r: &Run) -> HashMap<String, String> {
    r.read()
        .filter_map(|s| {
            let f = s.fetched.as_ref()?;
            Some((
                s.id.clone(),
                fs::read_to_string(dir.join(&f.text_file)).ok()?,
            ))
        })
        .collect()
}

// ---- claims --------------------------------------------------------------------------------

fn cite(arg: &str, claim: &str, source: &str, passage: &str, against: bool) -> anyhow::Result<()> {
    with_run(arg, |r, dir| {
        let s = r.source(source)?;
        let file = s
            .fetched
            .as_ref()
            .filter(|f| f.ok())
            .map(|f| dir.join(&f.text_file))
            .with_context(|| format!("{} has not been read", s.id))?;
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let stance = if against {
            Stance::Against
        } else {
            Stance::Supports
        };
        let c = r.cite(claim, source, passage, stance, &text, &clock::now())?;
        let f = &c.found;
        match f.verdict {
            Verdict::Exact => println!("verified (exact) at chars {}–{}", f.start, f.end),
            Verdict::Close => println!("verified (close, {:.0}% of words in order) at chars {}–{}:\n  \"{}\"", f.score * 100.0, f.start, f.end, f.excerpt),
            Verdict::TooShort => println!("NOT verified: quote at least 6 words so the passage can carry the claim"),
            Verdict::Partial => println!(
                "NOT verified: only {:.0}% of the words in order. Nearest text in {}:\n  \"{}\"\nQuote the source verbatim.",
                f.score * 100.0,
                c.source,
                f.excerpt
            ),
            Verdict::Missing => println!("NOT verified: the passage is not in {}. Quote the source verbatim.", c.source),
        }
        Ok(())
    })
}

fn status(arg: &str) -> anyhow::Result<()> {
    let (dir, r) = load(arg)?;
    let a = analyse(&r, &texts(&dir, &r));
    let md = render(&r, &a, clock::year());
    fs::write(dir.join("status.md"), &md)?;
    print!("{md}");
    Ok(())
}

fn export_run(arg: &str, about: &str, name: Option<&str>) -> anyhow::Result<()> {
    let (dir, r) = load(arg)?;
    let a = analyse(&r, &texts(&dir, &r));
    let pb = ProducedBy {
        organ: ORGAN.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        tool: None,
        model: None,
    };
    let e = export(
        &r,
        &a,
        about,
        name,
        &pb,
        &dir.to_string_lossy().replace('\\', "/"),
    );
    let body: String = e
        .messages
        .iter()
        .map(|m| serde_json::to_string(m).map(|s| s + "\n"))
        .collect::<Result<_, _>>()?;
    fs::write(dir.join("messages.jsonl"), body)?;
    fs::write(
        dir.join("proposal.json"),
        serde_json::to_string_pretty(&e.proposal)?,
    )?;
    let next = next_steps(&r, &a);
    println!("{}", dir.join("messages.jsonl").display());
    println!("{}", dir.join("proposal.json").display());
    println!(
        "{} messages; {} claims exported; dropped (no verified passage): {}",
        e.messages.len(),
        r.claims.len() - e.dropped.len(),
        if e.dropped.is_empty() {
            "none".into()
        } else {
            e.dropped.join(", ")
        }
    );
    if !next.is_empty() {
        println!("still open: {}", next.join("; "));
    }
    println!("then: mneme ingest messages.jsonl && mneme digest proposal.json (review the proposal first)");
    Ok(())
}

// ---- evaluation ----------------------------------------------------------------------------

fn eval_set(
    set_path: &Path,
    costs: Option<&Path>,
    out: Option<&Path>,
    episodes: Option<&Path>,
) -> anyhow::Result<()> {
    use organon_research_core::eval::{self, CaseScore, EvalSet, Measured, STAGES};
    let set = EvalSet::parse(
        &fs::read_to_string(set_path).with_context(|| format!("reading {}", set_path.display()))?,
    )
    .with_context(|| format!("parsing {}", set_path.display()))?;
    let costs: HashMap<String, Measured> = match costs {
        Some(p) => serde_json::from_str(
            &fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?,
        )
        .with_context(|| format!("parsing {}", p.display()))?,
        None => HashMap::new(),
    };
    let base = set_path.parent().unwrap_or_else(|| Path::new("."));
    let mut scores = Vec::new();
    let mut lines = String::new();
    for case in &set.cases {
        let Some(arg) = &case.run else {
            scores.push(CaseScore::not_run(case));
            continue;
        };
        // A relative run folder is read next to the set first, then as usual.
        let near = base.join(arg);
        let arg = if near.join("run.json").exists() {
            near.to_string_lossy().into_owned()
        } else {
            arg.clone()
        };
        let (dir, r) = load(&arg).with_context(|| format!("case {}", case.id))?;
        let a = analyse(&r, &texts(&dir, &r));
        let s = eval::score(
            case,
            &r,
            &a,
            costs.get(&case.id).cloned().unwrap_or_default(),
        );
        if episodes.is_some() {
            let (started, ended) = eval::span(&r);
            let m = organon_protocol::Message::Episode(eval::episode(&s, &started, &ended));
            lines.push_str(&serde_json::to_string(&m)?);
            lines.push('\n');
        }
        scores.push(s);
    }
    print!("{}", eval::render(&scores));
    if let Some(p) = out {
        let means: serde_json::Map<String, serde_json::Value> = STAGES
            .iter()
            .map(|st| {
                let (m, n) = eval::mean(&scores, st);
                ((*st).to_owned(), serde_json::json!({ "mean": m, "n": n }))
            })
            .collect();
        let body = serde_json::json!({
            "set": set_path.to_string_lossy(),
            "scored_at": clock::now(),
            "tool": concat!("organon-research@", env!("CARGO_PKG_VERSION")),
            "means": means,
            "cases": scores,
        });
        fs::write(p, serde_json::to_string_pretty(&body)?)?;
        println!("{}", p.display());
    }
    if let Some(p) = episodes {
        use std::io::Write as _;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .with_context(|| format!("opening {}", p.display()))?
            .write_all(lines.as_bytes())?;
        println!(
            "{} episodes appended to {}",
            lines.lines().count(),
            p.display()
        );
    }
    Ok(())
}

// ---- time ----------------------------------------------------------------------------------

mod clock {
    /// RFC 3339 UTC now, second precision.
    pub fn now() -> String {
        let (y, m, d, s) = parts();
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            s / 3600,
            s / 60 % 60,
            s % 60
        )
    }

    pub fn year() -> i32 {
        i32::try_from(parts().0).unwrap_or(2026)
    }

    /// (year, month, day, seconds of day), civil-from-days (Howard Hinnant).
    fn parts() -> (i64, i64, i64, u64) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let (days, rem) = (secs / 86_400, secs % 86_400);
        let z = i64::try_from(days).unwrap_or(0) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (yoe + era * 400 + i64::from(m <= 2), m, d, rem)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn now_is_rfc3339() {
            let t = super::now();
            assert_eq!(t.len(), 20);
            assert!(t.ends_with('Z') && t.as_bytes()[10] == b'T');
            assert!(super::year() >= 2026);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::dehyphenate;

    #[test]
    fn line_break_hyphens_join_real_hyphens_stay() {
        assert_eq!(dehyphenate("the accu-\nracy of OCR"), "the accuracy of OCR");
        assert_eq!(
            dehyphenate("state-of-the-art and\nCode-\nAware"),
            "state-of-the-art and\nCode-\nAware"
        );
    }
}

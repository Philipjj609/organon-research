//! HTTP: the only network code. Polite (identifying user agent, pauses per host, size cap).

use std::collections::HashMap;
use std::io::Read as _;
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _};

const USER_AGENT: &str = concat!(
    "organon-research/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/Philipjj609/organon-research)"
);
const MAX_BYTES: u64 = 6_000_000;

/// A response body and its content type.
pub struct Body {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

impl Body {
    /// The body as text (invalid UTF-8 replaced).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

pub struct Http {
    agent: ureq::Agent,
    last: HashMap<String, Instant>,
}

impl Http {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(30))
                .user_agent(USER_AGENT)
                .build(),
            last: HashMap::new(),
        }
    }

    /// GET `url`, waiting first so one host gets at most one request per `gap` (arXiv asks 3 s).
    pub fn get(&mut self, url: &str, accept: &str, gap: Duration) -> anyhow::Result<Body> {
        let host = organon_research_core::url::host(url).unwrap_or_default();
        if let Some(t) = self.last.get(&host) {
            let waited = t.elapsed();
            if waited < gap {
                std::thread::sleep(gap - waited);
            }
        }
        let result = self.agent.get(url).set("Accept", accept).call();
        self.last.insert(host, Instant::now());
        let resp = match result {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => bail!("HTTP {code}"),
            Err(e) => return Err(e).with_context(|| format!("GET {url}")),
        };
        let content_type = resp.content_type().to_owned();
        let mut buf = Vec::new();
        resp.into_reader()
            .take(MAX_BYTES)
            .read_to_end(&mut buf)
            .with_context(|| format!("reading {url}"))?;
        Ok(Body {
            bytes: buf,
            content_type,
        })
    }
}

/// Pause between requests to the same host.
pub fn gap_for(url: &str) -> Duration {
    let host = organon_research_core::url::host(url).unwrap_or_default();
    if host.ends_with("arxiv.org") {
        Duration::from_secs(3)
    } else {
        Duration::from_millis(500)
    }
}

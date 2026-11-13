//! Search backends: URL builders and response parsers for keyless APIs (the CLI does the HTTP).
//! General web search is left to the agent's own search tool; its results come in by `add`.

use serde_json::Value as J;

use crate::url::encode;
use crate::Error;

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub url: String,
    pub title: String,
    /// One line: year, venue, stars, points… whatever the backend offers.
    pub snippet: String,
}

/// A keyless search backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// Scholarly works (≈250M), keyless.
    OpenAlex,
    /// Preprints; keep to one request every 3 s.
    Arxiv,
    /// Hacker News stories via Algolia: practitioner discussion and dissent.
    HackerNews,
    /// GitHub repositories (keyless: 10 searches per minute).
    GitHub,
}

impl std::str::FromStr for Engine {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "openalex" | "papers" => Ok(Self::OpenAlex),
            "arxiv" => Ok(Self::Arxiv),
            "hn" | "hackernews" => Ok(Self::HackerNews),
            "github" | "gh" => Ok(Self::GitHub),
            other => Err(Error::Unknown(format!(
                "engine '{other}' (openalex, arxiv, hn, github)"
            ))),
        }
    }
}

impl Engine {
    pub const ALL: [Self; 4] = [Self::OpenAlex, Self::Arxiv, Self::HackerNews, Self::GitHub];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::OpenAlex => "openalex",
            Self::Arxiv => "arxiv",
            Self::HackerNews => "hn",
            Self::GitHub => "github",
        }
    }

    /// Request URL for `query`, at most `n` results.
    #[must_use]
    pub fn url(self, query: &str, n: usize) -> String {
        let q = encode(query);
        match self {
            Self::OpenAlex => format!(
                "https://api.openalex.org/works?search={q}&per-page={n}&select=id,doi,title,publication_year,primary_location,cited_by_count"
            ),
            Self::Arxiv => format!("https://export.arxiv.org/api/query?search_query=all:{q}&start=0&max_results={n}"),
            Self::HackerNews => format!("https://hn.algolia.com/api/v1/search?query={q}&tags=story&hitsPerPage={n}"),
            Self::GitHub => format!("https://api.github.com/search/repositories?q={q}&per_page={n}"),
        }
    }

    /// Parse a response body.
    ///
    /// # Errors
    /// The body is not the backend's format.
    pub fn parse(self, body: &str) -> Result<Vec<Hit>, Error> {
        match self {
            Self::Arxiv => Ok(parse_arxiv(body)),
            _ => {
                let j: J = serde_json::from_str(body)?;
                Ok(match self {
                    Self::OpenAlex => parse_openalex(&j),
                    Self::HackerNews => parse_hn(&j),
                    Self::GitHub => parse_github(&j),
                    Self::Arxiv => unreachable!("handled above"),
                })
            }
        }
    }
}

fn s(v: &J) -> &str {
    v.as_str().unwrap_or_default()
}

fn parse_openalex(j: &J) -> Vec<Hit> {
    j["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let loc = &w["primary_location"];
            let url = [
                s(&w["doi"]),
                s(&loc["landing_page_url"]),
                s(&loc["pdf_url"]),
                s(&w["id"]),
            ]
            .into_iter()
            .find(|u| !u.is_empty())?;
            let venue = s(&loc["source"]["display_name"]);
            let year = w["publication_year"]
                .as_u64()
                .map(|y| y.to_string())
                .unwrap_or_default();
            let cited = w["cited_by_count"].as_u64().unwrap_or(0);
            Some(Hit {
                url: url.to_owned(),
                title: s(&w["title"]).to_owned(),
                snippet: [year, venue.to_owned(), format!("cited by {cited}")]
                    .into_iter()
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
            })
        })
        .collect()
}

fn parse_hn(j: &J) -> Vec<Hit> {
    j["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let item = format!("https://news.ycombinator.com/item?id={}", s(&h["objectID"]));
            let url = Some(s(&h["url"]))
                .filter(|u| !u.is_empty())
                .map_or_else(|| item.clone(), str::to_owned);
            let date = s(&h["created_at"]).get(..10).unwrap_or_default().to_owned();
            Some(Hit {
                url,
                title: s(&h["title"]).to_owned(),
                snippet: format!(
                    "HN {} points, {} comments, {date} · discussion {item}",
                    h["points"].as_u64()?,
                    h["num_comments"].as_u64().unwrap_or(0)
                ),
            })
        })
        .collect()
}

fn parse_github(j: &J) -> Vec<Hit> {
    j["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| Hit {
            url: s(&r["html_url"]).to_owned(),
            title: s(&r["full_name"]).to_owned(),
            snippet: format!(
                "★{} · pushed {} · {}",
                r["stargazers_count"].as_u64().unwrap_or(0),
                s(&r["pushed_at"]).get(..10).unwrap_or_default(),
                s(&r["description"])
            ),
        })
        .collect()
}

/// arXiv's Atom feed, read with plain string search (entries are flat and regular).
fn parse_arxiv(xml: &str) -> Vec<Hit> {
    xml.split("<entry>")
        .skip(1)
        .filter_map(|e| {
            let tag = |name: &str| {
                let open = format!("<{name}>");
                let start = e.find(&open)? + open.len();
                let end = start + e[start..].find(&format!("</{name}>"))?;
                Some(
                    crate::text::decode(&e[start..end])
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            };
            let url = tag("id")?;
            let summary = tag("summary").unwrap_or_default();
            let date = tag("published").unwrap_or_default();
            Some(Hit {
                url,
                title: tag("title").unwrap_or_default(),
                snippet: format!(
                    "{} · {}",
                    date.get(..10).unwrap_or_default(),
                    summary.chars().take(160).collect::<String>()
                ),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openalex_prefers_doi_then_landing_page() {
        let body = r#"{"results":[{"id":"https://openalex.org/W1","doi":"https://doi.org/10.1/x","title":"Paper A","publication_year":2024,
            "primary_location":{"landing_page_url":"https://j.org/a","source":{"display_name":"Journal"}},"cited_by_count":12},
            {"id":"https://openalex.org/W2","doi":null,"title":"Paper B","primary_location":{"landing_page_url":"https://j.org/b"}}]}"#;
        let h = Engine::OpenAlex.parse(body).unwrap();
        assert_eq!(h[0].url, "https://doi.org/10.1/x");
        assert_eq!(h[0].snippet, "2024 · Journal · cited by 12");
        assert_eq!(h[1].url, "https://j.org/b");
    }

    #[test]
    fn hn_story_without_url_points_at_the_thread() {
        let body = r#"{"hits":[{"objectID":"42","title":"Ask HN: x","url":null,"points":10,"num_comments":3,"created_at":"2026-01-02T00:00:00Z"},
            {"objectID":"43","title":"Show","url":"https://a.com","points":5,"num_comments":0,"created_at":"2026-01-03T00:00:00Z"}]}"#;
        let h = Engine::HackerNews.parse(body).unwrap();
        assert_eq!(h[0].url, "https://news.ycombinator.com/item?id=42");
        assert_eq!(h[1].url, "https://a.com");
        assert!(h[1]
            .snippet
            .contains("discussion https://news.ycombinator.com/item?id=43"));
    }

    #[test]
    fn github_and_arxiv_parse() {
        let gh = r#"{"items":[{"html_url":"https://github.com/a/b","full_name":"a/b","stargazers_count":7,"pushed_at":"2026-05-01T00:00:00Z","description":"tool"}]}"#;
        assert_eq!(
            Engine::GitHub.parse(gh).unwrap()[0].snippet,
            "★7 · pushed 2026-05-01 · tool"
        );
        let xml = "<feed><entry><id>http://arxiv.org/abs/2605.06635v1</id><published>2026-05-07T00:00:00Z</published>\
            <title>Cited but Not\n  Verified</title><summary>We audit &amp; find gaps.</summary></entry></feed>";
        let h = Engine::Arxiv.parse(xml).unwrap();
        assert_eq!(
            (h[0].url.as_str(), h[0].title.as_str()),
            (
                "http://arxiv.org/abs/2605.06635v1",
                "Cited but Not Verified"
            )
        );
        assert_eq!(h[0].snippet, "2026-05-07 · We audit & find gaps.");
    }

    #[test]
    fn engines_by_name() {
        assert_eq!("hn".parse::<Engine>().unwrap(), Engine::HackerNews);
        assert!("google".parse::<Engine>().is_err());
        assert!(Engine::Arxiv
            .url("c++ rust", 5)
            .contains("all:c%2B%2B+rust"));
    }
}

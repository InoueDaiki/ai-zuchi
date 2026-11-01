//! 自前の SearXNG の JSON API。

use std::time::Duration;

use serde::Deserialize;
use ureq::Agent;
use zuchi_core::ports::{Result, Searcher};
use zuchi_core::types::SearchHit;

use crate::SearchError;
use crate::http::{DEFAULT_TIMEOUT, agent, is_web_url, map_error};

#[derive(Clone, Debug)]
pub struct SearxngConfig {
    /// 例: `http://127.0.0.1:8888`。本人が管理するインスタンスを指定する
    pub base_url: String,
    pub language: String,
    pub timeout: Duration,
}

impl SearxngConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            language: "ja".to_owned(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

pub struct SearxngSearcher {
    search_url: String,
    language: String,
    agent: Agent,
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    results: Vec<ResultItem>,
}

#[derive(Deserialize)]
struct ResultItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

impl SearxngSearcher {
    pub fn new(config: SearxngConfig) -> std::result::Result<Self, SearchError> {
        let base = config.base_url.trim().trim_end_matches('/');
        if !is_web_url(base) {
            return Err(SearchError::InvalidConfig(
                "SearXNG の URL は http:// か https:// で始める".to_owned(),
            ));
        }
        Ok(Self {
            search_url: format!("{base}/search"),
            language: config.language,
            agent: agent(config.timeout),
        })
    }

    fn fetch(&self, query: &str, limit: usize) -> std::result::Result<Vec<SearchHit>, SearchError> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut response = self
            .agent
            .get(&self.search_url)
            .query("q", query)
            .query("format", "json")
            .query("language", &self.language)
            .call()
            .map_err(map_error)?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(SearchError::Api {
                status,
                hint: if status == 403 {
                    "SearXNG の settings.yml の search.formats に json を足す"
                } else {
                    "SearXNG の状態を確かめる"
                },
            });
        }
        let body = response.body_mut().read_to_string().map_err(map_error)?;
        let parsed: Response =
            serde_json::from_str(&body).map_err(|e| SearchError::InvalidResponse(e.to_string()))?;
        Ok(parsed
            .results
            .into_iter()
            .filter_map(|item| {
                let url = item.url.filter(|u| is_web_url(u))?;
                (!item.title.trim().is_empty()).then(|| SearchHit {
                    title: item.title,
                    url,
                    snippet: item.content.unwrap_or_default(),
                })
            })
            .take(limit)
            .collect())
    }
}

impl Searcher for SearxngSearcher {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        Ok(self.fetch(query, limit)?)
    }
}

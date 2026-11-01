//! Brave Search API。環境変数 `BRAVE_SEARCH_API_KEY` があるときだけ有効にする。
//!
//! 規約で検索結果の保存が制限されているので、検索結果をデータベースに溜めない。
//! 詳しくは `docs/検証-検索.md` を参照する。

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use ureq::Agent;
use zuchi_core::ports::{Result, Searcher};
use zuchi_core::types::SearchHit;

use crate::http::{DEFAULT_TIMEOUT, agent, is_web_url, map_error, strip_tags};
use crate::{SearchError, non_empty_env};

pub const BRAVE_API_KEY_ENV: &str = "BRAVE_SEARCH_API_KEY";

const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
/// API の `count` の上限
const MAX_COUNT: usize = 20;

#[derive(Clone)]
pub struct BraveConfig {
    pub api_key: String,
    pub endpoint: String,
    pub country: String,
    pub timeout: Duration,
}

impl BraveConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            endpoint: ENDPOINT.to_owned(),
            country: "JP".to_owned(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// キーをログに出さないよう、手で書く
impl fmt::Debug for BraveConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BraveConfig")
            .field("api_key", &"<隠す>")
            .field("endpoint", &self.endpoint)
            .field("country", &self.country)
            .field("timeout", &self.timeout)
            .finish()
    }
}

pub struct BraveSearcher {
    config: BraveConfig,
    agent: Agent,
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    web: Option<WebResults>,
}

#[derive(Deserialize)]
struct WebResults {
    #[serde(default)]
    results: Vec<ResultItem>,
}

#[derive(Deserialize)]
struct ResultItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
}

impl BraveSearcher {
    pub fn new(config: BraveConfig) -> Self {
        let agent = agent(config.timeout);
        Self { config, agent }
    }

    pub fn from_env() -> Option<Self> {
        non_empty_env(BRAVE_API_KEY_ENV).map(|key| Self::new(BraveConfig::new(key.trim())))
    }

    fn fetch(&self, query: &str, limit: usize) -> std::result::Result<Vec<SearchHit>, SearchError> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let count = limit.min(MAX_COUNT).to_string();
        let mut response = self
            .agent
            .get(&self.config.endpoint)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.config.api_key)
            .query("q", query)
            .query("count", &count)
            .query("country", &self.config.country)
            .call()
            .map_err(map_error)?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(SearchError::Api {
                status,
                hint: match status {
                    401 | 403 => "BRAVE_SEARCH_API_KEY を確かめる",
                    429 => "回数かクレジットの上限に達した",
                    _ => "Brave Search API の状態を確かめる",
                },
            });
        }
        let body = response.body_mut().read_to_string().map_err(map_error)?;
        let parsed: Response =
            serde_json::from_str(&body).map_err(|e| SearchError::InvalidResponse(e.to_string()))?;
        Ok(parsed
            .web
            .map(|web| web.results)
            .unwrap_or_default()
            .into_iter()
            .filter(|item| is_web_url(&item.url) && !item.title.trim().is_empty())
            .map(|item| SearchHit {
                title: strip_tags(&item.title),
                url: item.url,
                snippet: strip_tags(&item.description),
            })
            .take(limit)
            .collect())
    }
}

impl Searcher for BraveSearcher {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        Ok(self.fetch(query, limit)?)
    }
}

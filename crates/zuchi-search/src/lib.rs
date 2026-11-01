//! Web 検索。外へ出るのは検索語だけ。
//!
//! 組み立ては外側から [`GuardedSearcher`]、[`HistorySearcher`]、実際の検索の順にする。
//! こうすると、履歴は「外へ出た検索語の完全な記録」になり、止めた語は履歴にも残らない。
//! 実装の選び方は `docs/検証-検索.md` を参照する。

mod brave;
mod guard;
mod history;
mod http;
mod searxng;

use std::env;
use std::fmt;

use zuchi_core::ports::Searcher;

pub use brave::{BRAVE_API_KEY_ENV, BraveConfig, BraveSearcher};
pub use guard::{DEFAULT_MAX_QUERY_CHARS, GuardedSearcher};
pub use history::{HistorySearcher, InMemorySearchLog, SearchLog, SearchRecord};
pub use searxng::{SearxngConfig, SearxngSearcher};

/// SearXNG の URL を指定する環境変数
pub const SEARXNG_URL_ENV: &str = "ZUCHI_SEARXNG_URL";

/// 検索語を含めない。止めた理由は呼び出し側が本人に見せる。
#[derive(Debug, PartialEq, Eq)]
pub enum SearchError {
    /// 除外語を含むので送らなかった
    Blocked,
    /// 文のように長い検索語なので送らなかった
    QueryTooLong {
        chars: usize,
        max: usize,
    },
    Network(String),
    Timeout,
    /// HTTP のエラー。`hint` は本人が直せるときの案内
    Api {
        status: u16,
        hint: &'static str,
    },
    InvalidResponse(String),
    InvalidConfig(String),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked => write!(f, "除外語を含むので検索しなかった"),
            Self::QueryTooLong { chars, max } => {
                write!(
                    f,
                    "検索語が長すぎるので送らなかった({chars} 文字、上限 {max})"
                )
            }
            Self::Network(detail) => write!(f, "検索の通信に失敗した: {detail}"),
            Self::Timeout => write!(f, "検索がタイムアウトした"),
            Self::Api { status, hint } => write!(f, "検索のエラー({status}): {hint}"),
            Self::InvalidResponse(detail) => write!(f, "検索の応答が不正: {detail}"),
            Self::InvalidConfig(detail) => write!(f, "検索の設定が不正: {detail}"),
        }
    }
}

impl std::error::Error for SearchError {}

/// 環境変数から検索の実装を選ぶ。SearXNG を優先し、どちらもなければ `None`(検索しない)。
pub fn searcher_from_env() -> Result<Option<Box<dyn Searcher>>, SearchError> {
    if let Some(url) = non_empty_env(SEARXNG_URL_ENV) {
        return Ok(Some(Box::new(SearxngSearcher::new(SearxngConfig::new(
            url,
        ))?)));
    }
    Ok(BraveSearcher::from_env().map(|s| Box::new(s) as Box<dyn Searcher>))
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

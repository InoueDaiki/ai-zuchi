//! 検索語が外へ出る前の関門。

use unicode_normalization::UnicodeNormalization;
use zuchi_core::ports::{Result, Searcher};
use zuchi_core::types::SearchHit;

use crate::SearchError;

/// 検索語は固有名詞と話題に絞る方針なので、これより長いものは会話の文の混入とみなす
pub const DEFAULT_MAX_QUERY_CHARS: usize = 64;

/// 除外語を含む検索語と、長すぎる検索語を止める。
pub struct GuardedSearcher<S> {
    inner: S,
    /// [`normalize`] を通した除外語
    blocked: Vec<String>,
    max_chars: usize,
}

impl<S: Searcher> GuardedSearcher<S> {
    pub fn new(inner: S, blocked_terms: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        Self {
            inner,
            blocked: blocked_terms
                .into_iter()
                .map(|term| normalize(term.as_ref()))
                .filter(|term| !term.is_empty())
                .collect(),
            max_chars: DEFAULT_MAX_QUERY_CHARS,
        }
    }

    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }

    fn check(&self, query: &str) -> std::result::Result<(), SearchError> {
        let chars = query.chars().count();
        if chars > self.max_chars {
            return Err(SearchError::QueryTooLong {
                chars,
                max: self.max_chars,
            });
        }
        let normalized = normalize(query);
        if self.blocked.iter().any(|term| normalized.contains(term)) {
            return Err(SearchError::Blocked);
        }
        Ok(())
    }
}

impl<S: Searcher> Searcher for GuardedSearcher<S> {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.check(query)?;
        self.inner.search(query, limit)
    }
}

/// 全角と半角、大文字と小文字、空白の違いですり抜けないようにそろえる。
fn normalize(text: &str) -> String {
    text.nfkc()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect()
}

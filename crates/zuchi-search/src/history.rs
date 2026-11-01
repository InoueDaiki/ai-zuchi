//! 外へ出た検索語の履歴。本人が見て確かめるためのもの。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::SystemTime;

use zuchi_core::ports::{Result, Searcher};
use zuchi_core::types::SearchHit;

/// 検索語だけを残す。検索結果は規約の制限があるので残さない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRecord {
    pub query: String,
    pub at: SystemTime,
}

/// 履歴の保存先。永続化は暗号化された記憶のストアにつなぐ。
pub trait SearchLog: Send + Sync {
    fn record(&self, record: &SearchRecord) -> Result<()>;
}

/// 検索語を送る前に記録する。記録できないときは送らない。
pub struct HistorySearcher<S, L> {
    inner: S,
    log: L,
}

impl<S: Searcher, L: SearchLog> HistorySearcher<S, L> {
    pub fn new(inner: S, log: L) -> Self {
        Self { inner, log }
    }
}

impl<S: Searcher, L: SearchLog> Searcher for HistorySearcher<S, L> {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.log.record(&SearchRecord {
            query: query.to_owned(),
            at: SystemTime::now(),
        })?;
        self.inner.search(query, limit)
    }
}

/// 上限つきのメモリ上の履歴。古いものから捨てる。
pub struct InMemorySearchLog {
    capacity: usize,
    records: Mutex<VecDeque<SearchRecord>>,
}

impl InMemorySearchLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            records: Mutex::new(VecDeque::new()),
        }
    }

    /// 古い順
    pub fn records(&self) -> Vec<SearchRecord> {
        self.lock().iter().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<SearchRecord>> {
        self.records.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SearchLog for InMemorySearchLog {
    fn record(&self, record: &SearchRecord) -> Result<()> {
        let mut records = self.lock();
        records.push_back(record.clone());
        while records.len() > self.capacity {
            records.pop_front();
        }
        Ok(())
    }
}

impl<L: SearchLog + ?Sized> SearchLog for std::sync::Arc<L> {
    fn record(&self, record: &SearchRecord) -> Result<()> {
        (**self).record(record)
    }
}

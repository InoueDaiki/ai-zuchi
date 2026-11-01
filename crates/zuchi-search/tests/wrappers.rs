//! 除外語と履歴のラッパーの検証。

use std::sync::{Arc, Mutex};

use zuchi_core::ports::{Result, Searcher};
use zuchi_core::types::SearchHit;
use zuchi_search::{
    GuardedSearcher, HistorySearcher, InMemorySearchLog, SearchError, SearchLog, SearchRecord,
};

/// 受け取った検索語を覚える偽の検索
#[derive(Clone, Default)]
struct Fake {
    sent: Arc<Mutex<Vec<String>>>,
}

impl Searcher for Fake {
    fn search(&self, query: &str, _limit: usize) -> Result<Vec<SearchHit>> {
        self.sent.lock().unwrap().push(query.to_owned());
        Ok(Vec::new())
    }
}

#[test]
fn 除外語を含む検索語は送らない() {
    let fake = Fake::default();
    let searcher = GuardedSearcher::new(fake.clone(), ["Project Falcon", "極秘案件"]);
    for query in [
        "project falcon 進捗",
        "PROJECTFALCON",
        "Ｐｒｏｊｅｃｔ　Ｆａｌｃｏｎ",
        "極秘案件 価格",
    ] {
        let error = searcher.search(query, 5).unwrap_err();
        assert_eq!(error.downcast_ref(), Some(&SearchError::Blocked), "{query}");
        assert!(!error.to_string().contains(query));
    }
    assert!(fake.sent.lock().unwrap().is_empty());

    searcher.search("BONX 差別化", 5).unwrap();
    assert_eq!(*fake.sent.lock().unwrap(), ["BONX 差別化"]);
}

#[test]
fn 空の除外語はすべてを止めない() {
    let searcher = GuardedSearcher::new(Fake::default(), ["", "  "]);
    assert!(searcher.search("BONX", 5).is_ok());
}

#[test]
fn 文のように長い検索語は送らない() {
    let fake = Fake::default();
    let searcher = GuardedSearcher::new(fake.clone(), [""; 0]).with_max_chars(5);
    let error = searcher.search("BONXの差別化", 5).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(SearchError::QueryTooLong { chars: 8, max: 5 })
    ));
    assert!(fake.sent.lock().unwrap().is_empty());
}

#[test]
fn 履歴は送った検索語だけを残す() {
    let fake = Fake::default();
    let log = Arc::new(InMemorySearchLog::new(10));
    let searcher = GuardedSearcher::new(
        HistorySearcher::new(fake.clone(), log.clone()),
        ["極秘案件"],
    );
    searcher.search("BONX 差別化", 5).unwrap();
    searcher.search("極秘案件 価格", 5).unwrap_err();

    let records = log.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].query, "BONX 差別化");
    assert_eq!(fake.sent.lock().unwrap().len(), 1);
}

#[test]
fn 履歴は上限を超えると古いものから捨てる() {
    let log = Arc::new(InMemorySearchLog::new(2));
    let searcher = HistorySearcher::new(Fake::default(), log.clone());
    for query in ["a", "b", "c"] {
        searcher.search(query, 1).unwrap();
    }
    let queries: Vec<_> = log.records().into_iter().map(|r| r.query).collect();
    assert_eq!(queries, ["b", "c"]);
}

struct BrokenLog;

impl SearchLog for BrokenLog {
    fn record(&self, _record: &SearchRecord) -> Result<()> {
        Err("書き込めない".into())
    }
}

#[test]
fn 履歴に残せないときは送らない() {
    let fake = Fake::default();
    let searcher = HistorySearcher::new(fake.clone(), BrokenLog);
    assert!(searcher.search("BONX", 5).is_err());
    assert!(fake.sent.lock().unwrap().is_empty());
}

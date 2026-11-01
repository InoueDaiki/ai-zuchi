//! 偽のサーバーで HTTP を検証する。外へは通信しない。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use serde_json::json;
use zuchi_core::ports::Searcher;
use zuchi_search::{BraveConfig, BraveSearcher, SearchError, SearxngConfig, SearxngSearcher};

/// 要求の先頭部分を返す。本文のない GET だけを扱う。
fn serve_once(status: &str, response: String) -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let status = status.to_owned();
    let (sender, receiver) = channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = Vec::new();
        let mut chunk = [0; 4096];
        while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).unwrap();
            assert_ne!(read, 0);
            buffer.extend_from_slice(&chunk[..read]);
        }
        sender
            .send(String::from_utf8_lossy(&buffer).into_owned())
            .unwrap();
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
            response.len()
        );
        let _ = stream.write_all(reply.as_bytes());
    });
    (url, receiver)
}

fn search_error(error: &zuchi_core::ports::Error) -> &SearchError {
    error.downcast_ref().expect("SearchError のはず")
}

#[test]
fn searxng_の応答を読む() {
    let (url, requests) = serve_once(
        "200 OK",
        json!({"results": [
            {"title": "BONX WORK", "url": "https://bonx.co/work/", "content": "特徴"},
            {"title": "本文なし", "url": "https://example.com/"},
            {"title": "危ないリンク", "url": "javascript:alert(1)", "content": "x"},
            {"title": "", "url": "https://example.com/no-title", "content": "x"},
            {"title": "上限の外", "url": "https://example.com/3", "content": "x"},
        ]})
        .to_string(),
    );
    let searcher = SearxngSearcher::new(SearxngConfig::new(format!("{url}/"))).unwrap();
    let hits = searcher.search("BONX 差別化", 2).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].url, "https://bonx.co/work/");
    assert_eq!(hits[0].snippet, "特徴");
    assert_eq!(hits[1].snippet, "");

    let head = requests.recv().unwrap();
    let line = head.lines().next().unwrap();
    assert!(line.starts_with("GET /search?"), "{line}");
    assert!(line.contains("format=json"), "{line}");
    assert!(line.contains("language=ja"), "{line}");
    assert!(line.contains("q=BONX"), "{line}");
    assert!(!line.contains('差'), "検索語は符号化する: {line}");
}

#[test]
fn searxng_の_403_は設定の案内を返す() {
    let (url, _requests) = serve_once("403 Forbidden", String::new());
    let searcher = SearxngSearcher::new(SearxngConfig::new(url)).unwrap();
    let error = searcher.search("x", 5).unwrap_err();
    assert!(matches!(
        search_error(&error),
        SearchError::Api { status: 403, .. }
    ));
    assert!(error.to_string().contains("settings.yml"));
}

#[test]
fn 空の検索語は送らない() {
    // 待ち受けていない URL でも、接続せずに返る
    let searcher = SearxngSearcher::new(SearxngConfig::new("http://127.0.0.1:1")).unwrap();
    assert!(searcher.search("  ", 5).unwrap().is_empty());
}

#[test]
fn searxng_の_url_を検査する() {
    assert!(SearxngSearcher::new(SearxngConfig::new("file:///etc/passwd")).is_err());
}

#[test]
fn 通信のエラーに検索語を含めない() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let searcher =
        SearxngSearcher::new(SearxngConfig::new(format!("http://127.0.0.1:{port}"))).unwrap();
    let error = searcher.search("社外秘の語", 5).unwrap_err();
    assert!(matches!(search_error(&error), SearchError::Network(_)));
    assert!(!error.to_string().contains("社外秘"));
}

#[test]
fn brave_の要求と応答() {
    let (url, requests) = serve_once(
        "200 OK",
        json!({"web": {"results": [
            {"title": "<strong>BONX</strong>", "url": "https://bonx.co/", "description": "<strong>IP</strong> インカム"},
        ]}})
        .to_string(),
    );
    let searcher = BraveSearcher::new(BraveConfig {
        endpoint: format!("{url}/res/v1/web/search"),
        ..BraveConfig::new("test-key")
    });
    let hits = searcher.search("BONX", 50).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "BONX");
    assert_eq!(hits[0].snippet, "IP インカム");

    let head = requests.recv().unwrap().to_ascii_lowercase();
    assert!(head.contains("x-subscription-token: test-key"), "{head}");
    assert!(head.contains("count=20"), "上限に丸める: {head}");
    assert!(head.contains("country=jp"), "{head}");
}

#[test]
fn brave_の結果が空でも読める() {
    let (url, _requests) = serve_once("200 OK", json!({"type": "search"}).to_string());
    let searcher = BraveSearcher::new(BraveConfig {
        endpoint: url,
        ..BraveConfig::new("test-key")
    });
    assert!(searcher.search("x", 5).unwrap().is_empty());
}

#[test]
fn brave_の_401_はキーの案内を返す() {
    let (url, _requests) = serve_once("401 Unauthorized", String::new());
    let searcher = BraveSearcher::new(BraveConfig {
        endpoint: url,
        ..BraveConfig::new("test-key")
    });
    let error = searcher.search("x", 5).unwrap_err();
    assert!(error.to_string().contains("BRAVE_SEARCH_API_KEY"));
}

#[test]
fn 設定の_debug_にキーを出さない() {
    let text = format!("{:?}", BraveConfig::new("secret-value"));
    assert!(!text.contains("secret-value"));
}

/// 実物の SearXNG で試す。`ZUCHI_SEARXNG_URL` がなければスキップする。
#[test]
fn 実物の_searxng_で検索する() {
    let Ok(url) = std::env::var(zuchi_search::SEARXNG_URL_ENV) else {
        eprintln!("スキップ: ZUCHI_SEARXNG_URL が未設定");
        return;
    };
    let searcher = SearxngSearcher::new(SearxngConfig::new(url)).unwrap();
    let hits = searcher.search("BONX 差別化", 5).unwrap();
    assert!(!hits.is_empty());
    for hit in hits {
        println!("{} {}", hit.title, hit.url);
    }
}

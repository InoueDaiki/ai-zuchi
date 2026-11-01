//! 偽のサーバーで HTTP のやり取りを検証する。実物の Ollama は要らない。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use zuchi_core::ports::{Llm, LlmRequest};
use zuchi_llm::{OllamaConfig, OllamaError, OllamaLlm};

struct Captured {
    head: String,
    body: String,
}

fn read_request(stream: &mut TcpStream) -> Captured {
    let mut buffer = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
            let length = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if buffer.len() >= end + 4 + length {
                let body = String::from_utf8_lossy(&buffer[end + 4..end + 4 + length]).into_owned();
                return Captured { head, body };
            }
        }
        let read = stream.read(&mut chunk).unwrap();
        assert_ne!(read, 0, "要求の途中で切れた");
        buffer.extend_from_slice(&chunk[..read]);
    }
}

/// 1 回だけ応答する。`delay` を置くとタイムアウトの検証に使える。
fn serve_once(status: &str, response: String, delay: Duration) -> (u16, Receiver<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let status = status.to_owned();
    let (sender, receiver) = channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let captured = read_request(&mut stream);
        sender.send(captured).unwrap();
        thread::sleep(delay);
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
            response.len()
        );
        let _ = stream.write_all(reply.as_bytes());
    });
    (port, receiver)
}

fn client(port: u16) -> OllamaLlm {
    OllamaLlm::new(OllamaConfig {
        port,
        ..OllamaConfig::new("chat-model", "embed-model")
    })
}

fn downcast(error: &zuchi_core::ports::Error) -> &OllamaError {
    error.downcast_ref().expect("OllamaError のはず")
}

#[test]
fn json_schema_は_format_へオブジェクトとして渡す() {
    let (port, requests) = serve_once(
        "200 OK",
        json!({"message": {"content": "{\"skill\":\"calc\"}"}, "done_reason": "stop"}).to_string(),
        Duration::ZERO,
    );
    let output = client(port)
        .complete(&LlmRequest {
            system: "固定の指示".into(),
            user: "3カ月で10億件".into(),
            json_schema: Some(r#"{"type":"object"}"#.into()),
            max_tokens: Some(64),
        })
        .unwrap();
    assert_eq!(output, r#"{"skill":"calc"}"#);

    let captured = requests.recv().unwrap();
    assert!(captured.head.starts_with("POST /api/chat "));
    let body: Value = serde_json::from_str(&captured.body).unwrap();
    assert_eq!(body["model"], "chat-model");
    assert_eq!(body["stream"], false);
    assert_eq!(body["format"], json!({"type": "object"}));
    assert_eq!(body["options"]["num_predict"], 64);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["content"], "3カ月で10億件");
}

#[test]
fn スキーマなしでは_format_を送らない() {
    let (port, requests) = serve_once(
        "200 OK",
        json!({"message": {"content": "こんにちは"}}).to_string(),
        Duration::ZERO,
    );
    let output = client(port)
        .complete(&LlmRequest {
            user: "やあ".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(output, "こんにちは");
    let body: Value = serde_json::from_str(&requests.recv().unwrap().body).unwrap();
    assert!(body.get("format").is_none());
    assert!(body.get("options").is_none());
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn 埋め込みは_api_embed_を使う() {
    let (port, requests) = serve_once(
        "200 OK",
        json!({"embeddings": [[0.25, -0.5]]}).to_string(),
        Duration::ZERO,
    );
    assert_eq!(client(port).embed("決定事項").unwrap(), vec![0.25, -0.5]);
    let captured = requests.recv().unwrap();
    assert!(captured.head.starts_with("POST /api/embed "));
    let body: Value = serde_json::from_str(&captured.body).unwrap();
    assert_eq!(body["model"], "embed-model");
    assert_eq!(body["input"], "決定事項");
}

#[test]
fn モデル未取得を区別する() {
    let (port, _requests) = serve_once(
        "404 Not Found",
        json!({"error": "model 'chat-model' not found"}).to_string(),
        Duration::ZERO,
    );
    let error = client(port).complete(&LlmRequest::default()).unwrap_err();
    let OllamaError::ModelNotFound { model } = downcast(&error) else {
        panic!("{error}");
    };
    assert_eq!(model, "chat-model");
    assert!(error.to_string().contains("ollama pull chat-model"));
}

#[test]
fn サーバーのエラーは本文の_error_を伝える() {
    let (port, _requests) = serve_once(
        "500 Internal Server Error",
        json!({"error": "out of memory"}).to_string(),
        Duration::ZERO,
    );
    let error = client(port).embed("x").unwrap_err();
    let OllamaError::Api { status, message } = downcast(&error) else {
        panic!("{error}");
    };
    assert_eq!(*status, 500);
    assert_eq!(message, "out of memory");
}

#[test]
fn 応答が遅いとタイムアウトする() {
    let (port, _requests) = serve_once("200 OK", "{}".to_owned(), Duration::from_secs(3));
    let llm = OllamaLlm::new(OllamaConfig {
        port,
        complete_timeout: Duration::from_millis(200),
        ..OllamaConfig::new("chat-model", "embed-model")
    });
    let error = llm.complete(&LlmRequest::default()).unwrap_err();
    assert!(matches!(downcast(&error), OllamaError::Timeout), "{error}");
}

#[test]
fn 接続できないことを区別する() {
    // 空いているポートを取ってから閉じる
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let error = client(port).embed("x").unwrap_err();
    assert!(
        matches!(downcast(&error), OllamaError::Unreachable(_)),
        "{error}"
    );
}

#[test]
fn 空の出力はエラーにする() {
    let (port, _requests) = serve_once(
        "200 OK",
        json!({"message": {"content": ""}, "done_reason": "length"}).to_string(),
        Duration::ZERO,
    );
    let error = client(port).complete(&LlmRequest::default()).unwrap_err();
    assert!(matches!(downcast(&error), OllamaError::InvalidResponse(_)));
}

#[test]
fn 途中で切れた_json_はエラーにする() {
    let (port, _requests) = serve_once(
        "200 OK",
        json!({"message": {"content": "{\"skill\":"}, "done_reason": "length"}).to_string(),
        Duration::ZERO,
    );
    let error = client(port)
        .complete(&LlmRequest {
            json_schema: Some("{}".into()),
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(downcast(&error), OllamaError::InvalidResponse(_)));
}

#[test]
fn 不正なスキーマは送る前に止める() {
    // 待ち受けていないポートでも、接続の前に失敗する
    let error = client(1)
        .complete(&LlmRequest {
            json_schema: Some("not json".into()),
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(downcast(&error), OllamaError::InvalidSchema(_)));
}

#[test]
fn リダイレクトを追わない() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        let reply = "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://example.invalid/api/chat\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(reply.as_bytes());
    });
    let error = client(port).complete(&LlmRequest::default()).unwrap_err();
    assert!(
        matches!(downcast(&error), OllamaError::Api { status: 307, .. }),
        "{error}"
    );
}

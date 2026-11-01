//! 手元の Ollama との結合テスト。Ollama やモデルがなければスキップする。
//!
//! 使うモデルは環境変数で指定する。
//! `ZUCHI_TEST_CHAT_MODEL=gemma4:e4b ZUCHI_TEST_EMBED_MODEL=bge-m3 cargo test -p zuchi-llm`

use std::env;

use zuchi_core::ports::{Llm, LlmRequest};
use zuchi_llm::{OllamaConfig, OllamaError, OllamaLlm};

fn running() -> Option<(OllamaLlm, Vec<String>)> {
    let llm = OllamaLlm::new(OllamaConfig::new(
        "zuchi-test-missing",
        "zuchi-test-missing",
    ));
    match llm.installed_models() {
        Ok(models) => Some((llm, models)),
        Err(error) => {
            eprintln!("スキップ: {error}");
            None
        }
    }
}

#[test]
fn 実物でも未取得のモデルを区別する() {
    let Some((llm, _)) = running() else { return };
    let error = llm.embed("テスト").unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(OllamaError::ModelNotFound { .. })
        ),
        "{error}"
    );
}

#[test]
fn 実物で構造化出力を受け取る() {
    let Some((_, models)) = running() else { return };
    let Ok(model) = env::var("ZUCHI_TEST_CHAT_MODEL") else {
        eprintln!("スキップ: ZUCHI_TEST_CHAT_MODEL が未設定");
        return;
    };
    if !models.contains(&model) {
        eprintln!("スキップ: {model} が未取得");
        return;
    }
    let llm = OllamaLlm::new(OllamaConfig::new(model, "unused"));
    let output = llm
        .complete(&LlmRequest {
            system: "JSON で答える".into(),
            user: "1 + 1 は?".into(),
            json_schema: Some(
                r#"{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"]}"#
                    .into(),
            ),
            max_tokens: Some(64),
        })
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert!(value["answer"].is_i64(), "{output}");
}

#[test]
fn 実物で埋め込みを受け取る() {
    let Some((_, models)) = running() else { return };
    let Ok(model) = env::var("ZUCHI_TEST_EMBED_MODEL") else {
        eprintln!("スキップ: ZUCHI_TEST_EMBED_MODEL が未設定");
        return;
    };
    if !models.contains(&model) {
        eprintln!("スキップ: {model} が未取得");
        return;
    }
    let llm = OllamaLlm::new(OllamaConfig::new("unused", model));
    assert!(!llm.embed("埋め込みのテスト").unwrap().is_empty());
}

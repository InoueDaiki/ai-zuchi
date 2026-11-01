//! [`Llm`] の Ollama 実装。
//!
//! 会話や画面の内容を渡すので、通信先を手元に固定する。接続先は設定で変えられるのが
//! ポート番号だけで、ホストは常に `127.0.0.1`。環境変数のプロキシとリダイレクトも使わない。

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ureq::Agent;
use zuchi_core::ports::{Llm, LlmRequest, Result};

/// Ollama の既定のポート
pub const DEFAULT_PORT: u16 = 11434;

/// 通信を手元に固定するため、ホストは定数にして設定に出さない。
const HOST: &str = "127.0.0.1";

/// 使うモデルとタイムアウト。モデルは検証で決めるので既定値を置かない。
#[derive(Clone, Debug)]
pub struct OllamaConfig {
    pub chat_model: String,
    pub embed_model: String,
    pub port: u16,
    pub connect_timeout: Duration,
    /// 生成の全体の上限。モデルの読み込みの時間も含む
    pub complete_timeout: Duration,
    pub embed_timeout: Duration,
}

impl OllamaConfig {
    pub fn new(chat_model: impl Into<String>, embed_model: impl Into<String>) -> Self {
        Self {
            chat_model: chat_model.into(),
            embed_model: embed_model.into(),
            port: DEFAULT_PORT,
            connect_timeout: Duration::from_secs(2),
            complete_timeout: Duration::from_secs(120),
            embed_timeout: Duration::from_secs(30),
        }
    }
}

/// 呼び出し側が場合分けできるように分ける。プロンプトの内容は含めない。
#[derive(Debug)]
pub enum OllamaError {
    /// Ollama が起動していない
    Unreachable(String),
    Timeout,
    /// `ollama pull` が必要
    ModelNotFound {
        model: String,
    },
    /// Ollama が返したエラー
    Api {
        status: u16,
        message: String,
    },
    /// 応答が想定の形でない。空の出力や、途中で切れた JSON を含む
    InvalidResponse(String),
    /// `LlmRequest::json_schema` が JSON でない
    InvalidSchema(String),
}

impl fmt::Display for OllamaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(detail) => write!(f, "Ollama に接続できない: {detail}"),
            Self::Timeout => write!(f, "Ollama の応答がタイムアウトした"),
            Self::ModelNotFound { model } => {
                write!(f, "モデル {model} がない。`ollama pull {model}` で取得する")
            }
            Self::Api { status, message } => {
                write!(f, "Ollama のエラー({status}): {message}")
            }
            Self::InvalidResponse(detail) => write!(f, "Ollama の応答が不正: {detail}"),
            Self::InvalidSchema(detail) => write!(f, "JSON Schema が不正: {detail}"),
        }
    }
}

impl std::error::Error for OllamaError {}

pub struct OllamaLlm {
    config: OllamaConfig,
    agent: Agent,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatOptions {
    num_predict: u32,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<ChatOptions>,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ChatResponseMessage,
    #[serde(default)]
    done_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: String,
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TagsModel>,
}

#[derive(Deserialize)]
struct TagsModel {
    name: String,
}

#[derive(Deserialize)]
struct ApiError {
    error: String,
}

impl OllamaLlm {
    pub fn new(config: OllamaConfig) -> Self {
        let agent = Agent::config_builder()
            // 既定では環境変数のプロキシを使うので、明示的に切る
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(config.connect_timeout))
            .build()
            .new_agent();
        Self { config, agent }
    }

    /// 取得済みのモデルの名前。起動の確認にも使う。
    pub fn installed_models(&self) -> std::result::Result<Vec<String>, OllamaError> {
        let body = self.send(
            self.agent
                .get(self.url("/api/tags"))
                .config()
                .timeout_global(Some(self.config.connect_timeout + Duration::from_secs(3)))
                .build()
                .call(),
            "",
        )?;
        let tags: TagsResponse = parse(&body)?;
        Ok(tags.models.into_iter().map(|m| m.name).collect())
    }

    fn url(&self, path: &str) -> String {
        format!("http://{HOST}:{}{path}", self.config.port)
    }

    fn post(
        &self,
        path: &str,
        model: &str,
        body: &impl Serialize,
        timeout: Duration,
    ) -> std::result::Result<String, OllamaError> {
        let json =
            serde_json::to_string(body).map_err(|e| OllamaError::InvalidResponse(e.to_string()))?;
        let response = self
            .agent
            .post(self.url(path))
            .config()
            .timeout_global(Some(timeout))
            .build()
            .header("Content-Type", "application/json")
            .send(&json);
        self.send(response, model)
    }

    fn send(
        &self,
        response: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
        model: &str,
    ) -> std::result::Result<String, OllamaError> {
        let mut response = response.map_err(map_transport_error)?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(map_transport_error)?;
        match status {
            200..=299 => Ok(body),
            404 => Err(OllamaError::ModelNotFound {
                model: model.to_owned(),
            }),
            _ => {
                let message = serde_json::from_str::<ApiError>(&body)
                    .map(|e| e.error)
                    .unwrap_or(body);
                Err(OllamaError::Api { status, message })
            }
        }
    }

    fn chat(&self, request: &LlmRequest) -> std::result::Result<String, OllamaError> {
        let format = request
            .json_schema
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()
            .map_err(|e| OllamaError::InvalidSchema(e.to_string()))?;
        let wants_json = format.is_some();

        let mut messages = Vec::with_capacity(2);
        if !request.system.is_empty() {
            messages.push(ChatMessage {
                role: "system",
                content: &request.system,
            });
        }
        messages.push(ChatMessage {
            role: "user",
            content: &request.user,
        });
        let body = ChatRequest {
            model: &self.config.chat_model,
            messages,
            stream: false,
            format,
            options: request
                .max_tokens
                .map(|num_predict| ChatOptions { num_predict }),
        };

        let raw = self.post(
            "/api/chat",
            &self.config.chat_model,
            &body,
            self.config.complete_timeout,
        )?;
        let response: ChatResponse = parse(&raw)?;
        let content = response.message.content;
        if content.trim().is_empty() {
            return Err(OllamaError::InvalidResponse(format!(
                "出力が空(done_reason: {:?})",
                response.done_reason
            )));
        }
        // 途中で切れた JSON を、判定の結果として渡さない
        if wants_json && serde_json::from_str::<Value>(&content).is_err() {
            return Err(OllamaError::InvalidResponse(format!(
                "JSON として読めない(done_reason: {:?})",
                response.done_reason
            )));
        }
        Ok(content)
    }

    fn embedding(&self, text: &str) -> std::result::Result<Vec<f32>, OllamaError> {
        let raw = self.post(
            "/api/embed",
            &self.config.embed_model,
            &EmbedRequest {
                model: &self.config.embed_model,
                input: text,
            },
            self.config.embed_timeout,
        )?;
        let response: EmbedResponse = parse(&raw)?;
        response
            .embeddings
            .into_iter()
            .next()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| OllamaError::InvalidResponse("埋め込みが空".to_owned()))
    }
}

impl Llm for OllamaLlm {
    fn complete(&self, request: &LlmRequest) -> Result<String> {
        Ok(self.chat(request)?)
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.embedding(text)?)
    }
}

fn parse<'a, T: Deserialize<'a>>(raw: &'a str) -> std::result::Result<T, OllamaError> {
    // serde_json のエラーは位置だけで、本文を含まない
    serde_json::from_str(raw).map_err(|e| OllamaError::InvalidResponse(e.to_string()))
}

fn map_transport_error(error: ureq::Error) -> OllamaError {
    match error {
        // Windows は閉じたポートへの接続の拒否に約 2 秒かかり、接続のタイムアウトとして現れる
        ureq::Error::Timeout(ureq::Timeout::Connect) => {
            OllamaError::Unreachable("接続がタイムアウトした".to_owned())
        }
        ureq::Error::Timeout(_) => OllamaError::Timeout,
        other => OllamaError::Unreachable(other.to_string()),
    }
}

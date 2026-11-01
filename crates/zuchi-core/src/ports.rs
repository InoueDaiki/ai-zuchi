//! 外部との境界。実装は別のクレートに置き、テストでは Fake に差し替える。
//!
//! 処理はスレッドと channel でつなぐので、trait は同期で書く。

use std::sync::mpsc::Sender;

use crate::types::{
    AudioFrame, Evidence, Reaction, ScreenText, SearchHit, SpeechSegment, Suggestion,
};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// 止めると取得も止まる。drop でも止まるようにする。
pub trait CaptureHandle: Send {
    fn stop(self: Box<Self>);
}

/// マイクや PC の音声。OS ごとに実装する。
pub trait AudioSource: Send {
    fn start(&mut self, sink: Sender<AudioFrame>) -> Result<Box<dyn CaptureHandle>>;
}

/// 音声の断片から発話区間を切り出す。
pub trait SpeechDetector: Send {
    fn push(&mut self, frame: &AudioFrame) -> Vec<SpeechSegment>;
}

/// 発話区間を文字にする。
pub trait Transcriber: Send {
    fn transcribe(&mut self, segment: &SpeechSegment) -> Result<String>;
}

/// 画面の変化を検知し、変わった文字を流す。自分のオーバーレイと対象外のアプリは読まない。
pub trait ScreenSource: Send {
    fn start(&mut self, sink: Sender<ScreenText>) -> Result<Box<dyn CaptureHandle>>;
}

#[derive(Clone, Debug, Default)]
pub struct LlmRequest {
    /// 固定の指示。外部の内容を混ぜない
    pub system: String,
    /// 会話や画面の内容。データとして扱う
    pub user: String,
    /// 指定すると、この JSON Schema に沿った文字列を返す
    pub json_schema: Option<String>,
    pub max_tokens: Option<u32>,
}

pub trait Llm: Send + Sync {
    fn complete(&self, request: &LlmRequest) -> Result<String>;
    /// 検索用の埋め込み
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// Web 検索。渡せるのは検索語だけで、会話の文脈を渡す口を作らない。
pub trait Searcher: Send + Sync {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recalled {
    pub text: String,
    pub evidence: Evidence,
    pub score: f32,
}

/// 長期の記憶。
pub trait MemoryStore: Send + Sync {
    fn remember_utterance(&self, utterance: &crate::types::Utterance) -> Result<()>;
    fn remember_screen(&self, screen: &ScreenText) -> Result<()>;
    fn remember_reaction(&self, suggestion: &Suggestion, reaction: Reaction) -> Result<()>;
    fn recall(&self, query: &str, limit: usize) -> Result<Vec<Recalled>>;
}

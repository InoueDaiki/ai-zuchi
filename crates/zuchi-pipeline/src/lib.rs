//! 発話と画面の文字を受け取り、ルール、LLM の判定、スキル、確認の順に通して補足を作る。

pub mod engine;
pub mod prompts;
pub mod rules;
pub mod skills;

pub use engine::{Engine, EngineConfig, Input};

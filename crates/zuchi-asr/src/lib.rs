//! `zuchi-core` の `SpeechDetector` と `Transcriber` を sherpa-onnx で実装する。
//!
//! VAD は silero、ASR は日本語 zipformer(reazonspeech int8)を発話区間ごとに使う。
//! 設定の根拠は `docs/検証-文字起こし.md`。音声はメモリの中だけで扱い、ディスクに書かない。

mod detector;
mod models;
mod ring;
mod transcriber;

pub use detector::{SileroDetector, VadConfig};
pub use models::ModelPaths;
pub use transcriber::{AsrConfig, ZipformerTranscriber};

/// 扱う音声のサンプリング周波数。`AudioFrame` と同じ。
pub const SAMPLE_RATE: u32 = 16_000;

fn secs_to_samples(secs: f32) -> usize {
    (secs * SAMPLE_RATE as f32).round() as usize
}

//! 日本語 zipformer による文字起こし。

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use zuchi_core::ports::{Result, Transcriber};
use zuchi_core::types::SpeechSegment;

use crate::models::checked_path;
use crate::{ModelPaths, SAMPLE_RATE, secs_to_samples};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AsrConfig {
    /// 区間の前後に足す無音
    pub silence_secs: f32,
    /// 文字がこの数より少ない結果は、雑音によるものとして捨てる。句読点は数えない
    pub min_chars: usize,
    pub num_threads: i32,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            silence_secs: 0.3,
            min_chars: 3,
            // 4 スレッドにしても 2 割ほどしか縮まないので、会議のアプリに CPU を譲る
            num_threads: 1,
        }
    }
}

/// `Transcriber` の zipformer 実装。雑音として捨てた区間は空文字列を返す。
pub struct ZipformerTranscriber {
    recognizer: OfflineRecognizer,
    silence: Vec<f32>,
    min_chars: usize,
}

impl ZipformerTranscriber {
    pub fn new(paths: &ModelPaths, config: AsrConfig) -> Result<Self> {
        let mut recognizer_config = OfflineRecognizerConfig::default();
        recognizer_config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: Some(checked_path(&paths.encoder)?),
            decoder: Some(checked_path(&paths.decoder)?),
            joiner: Some(checked_path(&paths.joiner)?),
        };
        recognizer_config.model_config.tokens = Some(checked_path(&paths.tokens)?);
        recognizer_config.model_config.num_threads = config.num_threads;
        recognizer_config.model_config.provider = Some("cpu".into());
        recognizer_config.decoding_method = Some("greedy_search".into());
        let recognizer =
            OfflineRecognizer::create(&recognizer_config).ok_or("ASR を作れなかった")?;
        Ok(Self {
            recognizer,
            silence: vec![0.0; secs_to_samples(config.silence_secs)],
            min_chars: config.min_chars,
        })
    }
}

impl Transcriber for ZipformerTranscriber {
    fn transcribe(&mut self, segment: &SpeechSegment) -> Result<String> {
        if segment.samples.is_empty() {
            return Ok(String::new());
        }
        let stream = self.recognizer.create_stream();
        let rate = SAMPLE_RATE as i32;
        stream.accept_waveform(rate, &self.silence);
        stream.accept_waveform(rate, &segment.samples);
        stream.accept_waveform(rate, &self.silence);
        self.recognizer.decode(&stream);
        let result = stream.get_result().ok_or("ASR の結果を読めなかった")?;
        Ok(drop_noise(result.text.trim(), self.min_chars))
    }
}

/// VAD の誤検知でも zipformer は平均 1.8 文字ほどを出すので、短い結果を捨てる。
/// 「はい」のような短い相づちも一緒に落ちるが、迷ったら黙る方針に合わせる。
fn drop_noise(text: &str, min_chars: usize) -> String {
    if text.chars().filter(|c| c.is_alphanumeric()).count() < min_chars {
        String::new()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 短い結果は_捨てる() {
        assert_eq!(drop_noise("ん", 3), "");
        assert_eq!(drop_noise("はい。", 3), "");
        assert_eq!(drop_noise("、。", 3), "");
    }

    #[test]
    fn 十分な長さの結果は_残す() {
        assert_eq!(drop_noise("10億件", 3), "10億件");
    }

    #[test]
    fn しきい値が_0_なら_何も捨てない() {
        assert_eq!(drop_noise("ん", 0), "ん");
    }
}

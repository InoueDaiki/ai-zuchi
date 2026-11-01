//! silero VAD による発話区間の切り出し。

use std::time::{Duration, SystemTime};

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use zuchi_core::ports::{Result, SpeechDetector};
use zuchi_core::types::{AudioChannel, AudioFrame, SpeechSegment};

use crate::models::checked_path;
use crate::ring::AudioRing;
use crate::{ModelPaths, SAMPLE_RATE, secs_to_samples};

/// silero の窓の長さ(サンプル数)。16 kHz では 512 に固定されている。
const WINDOW: usize = 512;

/// 事前の検証で選んだ値を既定にする。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VadConfig {
    pub threshold: f32,
    /// 区切りとみなす無音。0.5〜0.8 秒を想定する。長いほど CER はよいが、遅れは増える
    pub min_silence_secs: f32,
    pub min_speech_secs: f32,
    /// これを超える発話は途中で切る
    pub max_speech_secs: f32,
    /// 区間の前後に足す実音声
    pub context_secs: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            min_silence_secs: 0.8,
            min_speech_secs: 0.25,
            max_speech_secs: 20.0,
            context_secs: 0.3,
        }
    }
}

/// `SpeechDetector` の silero 実装。状態を持つので、チャンネルごとに 1 つ作る。
///
/// 返す区間の `samples` には前後の実音声が含まれる。`started_at` と `ended_at` は
/// 足す前の、VAD が判定した発話の時刻を指す。
pub struct SileroDetector {
    vad: VoiceActivityDetector,
    ring: AudioRing,
    context: u64,
    /// 通算の位置と時刻の対応。最新のフレームの先頭を基準にする
    anchor: Option<(u64, SystemTime, AudioChannel)>,
}

impl SileroDetector {
    pub fn new(paths: &ModelPaths, config: VadConfig) -> Result<Self> {
        let model_config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(checked_path(&paths.vad)?),
                threshold: config.threshold,
                min_silence_duration: config.min_silence_secs,
                min_speech_duration: config.min_speech_secs,
                window_size: WINDOW as i32,
                max_speech_duration: config.max_speech_secs,
            },
            sample_rate: SAMPLE_RATE as i32,
            num_threads: 1,
            provider: Some("cpu".into()),
            ..Default::default()
        };
        // 区間の開始から確定までの音声が残るだけの長さを持つ
        let keep_secs =
            config.max_speech_secs + config.min_silence_secs + 2.0 * config.context_secs;
        let vad = VoiceActivityDetector::create(&model_config, keep_secs + 1.0)
            .ok_or("VAD を作れなかった")?;
        Ok(Self {
            vad,
            ring: AudioRing::new(secs_to_samples(keep_secs + 1.0)),
            context: secs_to_samples(config.context_secs) as u64,
            anchor: None,
        })
    }

    /// 音声の終わりに、途中の区間を確定して返す。その後は新しい音声として使える。
    ///
    /// `SpeechDetector` には終わりを伝える口がないので、ここに置く。
    pub fn flush(&mut self) -> Vec<SpeechSegment> {
        let mut segments = Vec::new();
        self.vad.flush();
        self.drain(&mut segments);
        self.vad.reset();
        self.ring.clear();
        self.anchor = None;
        segments
    }

    fn drain(&mut self, segments: &mut Vec<SpeechSegment>) {
        let Some((anchor_pos, anchor_time, channel)) = self.anchor else {
            return;
        };
        while let Some(found) = self.vad.front() {
            let start = found.start().max(0) as u64;
            let end = start + found.n().max(0) as u64;
            let samples = self
                .ring
                .range(start.saturating_sub(self.context), end + self.context);
            segments.push(SpeechSegment {
                channel,
                samples,
                started_at: time_at(anchor_pos, anchor_time, start),
                ended_at: time_at(anchor_pos, anchor_time, end),
            });
            self.vad.pop();
        }
    }
}

impl SpeechDetector for SileroDetector {
    fn push(&mut self, frame: &AudioFrame) -> Vec<SpeechSegment> {
        self.anchor = Some((self.ring.end(), frame.captured_at, frame.channel));
        let mut segments = Vec::new();
        // 大きなフレームでも、区間が確定した時点の音声をリングに残すため、細かく分けて渡す
        for chunk in frame.samples.chunks(WINDOW) {
            self.ring.push(chunk);
            self.vad.accept_waveform(chunk);
            self.drain(&mut segments);
        }
        segments
    }
}

/// `captured_at` はフレームの先頭の時刻として扱う。
fn time_at(anchor_pos: u64, anchor_time: SystemTime, pos: u64) -> SystemTime {
    let to_duration = |samples: u64| Duration::from_secs_f64(samples as f64 / SAMPLE_RATE as f64);
    if pos >= anchor_pos {
        anchor_time + to_duration(pos - anchor_pos)
    } else {
        anchor_time
            .checked_sub(to_duration(anchor_pos - pos))
            .unwrap_or(anchor_time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 位置から_時刻を_求める() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        assert_eq!(
            time_at(16_000, base, 24_000),
            base + Duration::from_millis(500)
        );
        assert_eq!(
            time_at(16_000, base, 8_000),
            base - Duration::from_millis(500)
        );
    }
}

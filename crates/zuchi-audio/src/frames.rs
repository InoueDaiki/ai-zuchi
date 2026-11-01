//! 取得したままの音声を、16 kHz モノラルの固定長の [`AudioFrame`] にする。

use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime};

use zuchi_core::types::{AudioChannel, AudioFrame};

use crate::resample::{Resampler, TARGET_RATE};

/// 1 フレームのサンプル数(32 ms)。silero VAD の 16 kHz の窓に合わせる。
pub const FRAME_SAMPLES: usize = 512;

/// 受け手がいなくなった。取得を止める合図。
#[derive(Debug, PartialEq, Eq)]
pub struct SinkClosed;

/// インターリーブの音声を受け取り、フレームにして送る。
pub struct FrameBuilder {
    channel: AudioChannel,
    sink: Sender<AudioFrame>,
    format: Option<(u32, usize)>,
    resampler: Resampler,
    mono: Vec<f32>,
    pending: Vec<f32>,
}

impl FrameBuilder {
    pub fn new(channel: AudioChannel, sink: Sender<AudioFrame>) -> Self {
        Self {
            channel,
            sink,
            format: None,
            resampler: Resampler::new(TARGET_RATE, TARGET_RATE),
            mono: Vec::new(),
            pending: Vec::with_capacity(FRAME_SAMPLES * 2),
        }
    }

    pub fn push(&mut self, samples: &[f32], rate: u32, channels: usize) -> Result<(), SinkClosed> {
        if channels == 0 || rate == 0 {
            return Ok(());
        }
        // 出力先の切り替えなどで形式が変わることがある
        if self.format != Some((rate, channels)) {
            self.format = Some((rate, channels));
            self.resampler = Resampler::new(rate, TARGET_RATE);
        }

        self.mono.clear();
        self.mono.extend(
            samples
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
        self.resampler.process(&self.mono, &mut self.pending);

        let mut sent = 0;
        while self.pending.len() - sent >= FRAME_SAMPLES {
            let frame = self.pending[sent..sent + FRAME_SAMPLES].to_vec();
            sent += FRAME_SAMPLES;
            self.send(frame)?;
        }
        self.pending.drain(..sent);
        Ok(())
    }

    fn send(&self, samples: Vec<f32>) -> Result<(), SinkClosed> {
        let duration = Duration::from_secs_f64(samples.len() as f64 / f64::from(TARGET_RATE));
        let frame = AudioFrame {
            channel: self.channel,
            samples,
            // 先頭のサンプルのおおよその時刻
            captured_at: SystemTime::now() - duration,
        };
        self.sink.send(frame).map_err(|_| SinkClosed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    #[test]
    fn 固定長のフレームに切り分ける() {
        let (tx, rx) = channel();
        let mut builder = FrameBuilder::new(AudioChannel::Mic, tx);
        builder.push(&vec![0.25; 700], TARGET_RATE, 1).unwrap();
        builder.push(&vec![0.25; 400], TARGET_RATE, 1).unwrap();
        let frames: Vec<_> = rx.try_iter().collect();
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.samples.len() == FRAME_SAMPLES));
        assert!(frames.iter().all(|f| f.channel == AudioChannel::Mic));
    }

    #[test]
    fn 複数チャンネルを平均してモノラルにする() {
        let (tx, rx) = channel();
        let mut builder = FrameBuilder::new(AudioChannel::System, tx);
        let stereo: Vec<f32> = [1.0, 0.0].repeat(FRAME_SAMPLES);
        builder.push(&stereo, TARGET_RATE, 2).unwrap();
        let frame = rx.try_recv().unwrap();
        assert!(frame.samples.iter().all(|&s| s == 0.5));
    }

    #[test]
    fn 途中で形式が変わっても処理を続ける() {
        let (tx, rx) = channel();
        let mut builder = FrameBuilder::new(AudioChannel::System, tx);
        builder.push(&vec![0.5; 48_000], 48_000, 1).unwrap();
        builder.push(&vec![0.5; 88_200], 44_100, 2).unwrap();
        let total: usize = rx.try_iter().map(|f| f.samples.len()).sum();
        assert!(total.abs_diff(32_000) < 1_024, "{total}");
    }

    #[test]
    fn 受け手がいなければ知らせる() {
        let (tx, rx) = channel();
        drop(rx);
        let mut builder = FrameBuilder::new(AudioChannel::Mic, tx);
        let result = builder.push(&vec![0.0; FRAME_SAMPLES], TARGET_RATE, 1);
        assert_eq!(result, Err(SinkClosed));
    }
}

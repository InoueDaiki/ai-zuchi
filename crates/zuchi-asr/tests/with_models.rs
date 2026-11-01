//! モデルを使う結合テスト。`models/` がなければ何もせず通る(CI ではモデルを取得しない)。
//! 音声はモデルの配布物に含まれる `test_wavs/` を使う。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sherpa_onnx::Wave;
use zuchi_asr::{AsrConfig, ModelPaths, SileroDetector, VadConfig, ZipformerTranscriber};
use zuchi_core::ports::{SpeechDetector, Transcriber};
use zuchi_core::types::{AudioChannel, AudioFrame, SpeechSegment};

const RATE: usize = 16_000;

fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models")
}

fn paths() -> Option<ModelPaths> {
    let paths = ModelPaths::in_dir(models_dir());
    if !paths.exist() {
        eprintln!("モデルがないのでスキップする");
        return None;
    }
    Some(paths)
}

fn read_sample(name: &str) -> Vec<f32> {
    let path = models_dir()
        .join("sherpa-onnx-zipformer-ja-reazonspeech-2024-08-01/test_wavs")
        .join(name);
    let wave = Wave::read(path.to_str().unwrap()).unwrap();
    assert_eq!(wave.sample_rate() as usize, RATE);
    wave.samples().to_vec()
}

fn with_silence(before_secs: f32, samples: &[f32], after_secs: f32) -> Vec<f32> {
    let mut audio = vec![0.0; (before_secs * RATE as f32) as usize];
    audio.extend_from_slice(samples);
    audio.resize(audio.len() + (after_secs * RATE as f32) as usize, 0.0);
    audio
}

fn detect(detector: &mut SileroDetector, audio: &[f32], base: SystemTime) -> Vec<SpeechSegment> {
    let mut segments = Vec::new();
    for (index, chunk) in audio.chunks(RATE / 10).enumerate() {
        segments.extend(detector.push(&AudioFrame {
            channel: AudioChannel::Mic,
            samples: chunk.to_vec(),
            captured_at: base + Duration::from_millis(index as u64 * 100),
        }));
    }
    segments.extend(detector.flush());
    segments
}

#[test]
fn wav_を_文字にできる() {
    let Some(paths) = paths() else { return };
    let audio = with_silence(1.0, &read_sample("5.wav"), 2.0);
    let mut detector = SileroDetector::new(&paths, VadConfig::default()).unwrap();
    let mut transcriber = ZipformerTranscriber::new(&paths, AsrConfig::default()).unwrap();

    let segments = detect(&mut detector, &audio, SystemTime::now());
    let text: String = segments
        .iter()
        .map(|segment| transcriber.transcribe(segment).unwrap())
        .collect();

    assert!(text.contains("持ち主"), "結果: {text}");
    assert!(text.contains("看板"), "結果: {text}");
    assert_eq!(segments[0].channel, AudioChannel::Mic);
}

#[test]
fn 無音からは_区間が_出ない() {
    let Some(paths) = paths() else { return };
    let mut detector = SileroDetector::new(&paths, VadConfig::default()).unwrap();
    let segments = detect(&mut detector, &vec![0.0; RATE * 3], SystemTime::now());
    assert!(segments.is_empty());
}

#[test]
fn 区間の位置は_入力の音声と_一致する() {
    let Some(paths) = paths() else { return };
    let config = VadConfig {
        context_secs: 0.0,
        ..VadConfig::default()
    };
    let audio = with_silence(2.0, &read_sample("3.wav"), 2.0);
    let mut detector = SileroDetector::new(&paths, config).unwrap();
    let base = SystemTime::now();

    let segments = detect(&mut detector, &audio, base);

    let segment = &segments[0];
    let offset = segment
        .started_at
        .duration_since(base)
        .unwrap()
        .as_secs_f64();
    let start = (offset * RATE as f64).round() as usize;
    assert!(offset > 1.5, "開始 {offset} 秒");
    assert_eq!(
        segment.samples[..],
        audio[start..start + segment.samples.len()]
    );
}

#[test]
fn 前後の実音声が_足される() {
    let Some(paths) = paths() else { return };
    let audio = with_silence(2.0, &read_sample("3.wav"), 2.0);
    let base = SystemTime::now();

    let bare_config = VadConfig {
        context_secs: 0.0,
        ..VadConfig::default()
    };
    let mut bare = SileroDetector::new(&paths, bare_config).unwrap();
    let bare_segments = detect(&mut bare, &audio, base);

    let mut padded = SileroDetector::new(&paths, VadConfig::default()).unwrap();
    let padded_segments = detect(&mut padded, &audio, base);

    assert_eq!(bare_segments.len(), padded_segments.len());
    let added = padded_segments[0].samples.len() - bare_segments[0].samples.len();
    assert_eq!(added, RATE * 6 / 10);
}

#[test]
fn flush_の後も_使い続けられる() {
    let Some(paths) = paths() else { return };
    let audio = with_silence(1.0, &read_sample("3.wav"), 1.0);
    let mut detector = SileroDetector::new(&paths, VadConfig::default()).unwrap();
    let base = SystemTime::now();

    let first = detect(&mut detector, &audio, base);
    let second = detect(&mut detector, &audio, base);

    assert_eq!(first.len(), second.len());
    assert_eq!(first[0].samples, second[0].samples);
}

#[test]
fn 長い発話は_最大の長さで_切る() {
    let Some(paths) = paths() else { return };
    let speech = read_sample("1.wav");
    let audio: Vec<f32> = speech.iter().copied().cycle().take(RATE * 45).collect();
    let mut detector = SileroDetector::new(&paths, VadConfig::default()).unwrap();

    let segments = detect(&mut detector, &audio, SystemTime::now());

    assert!(segments.len() >= 2);
    for segment in segments {
        // 20 秒に、前後の 0.3 秒ずつと、余裕を足した長さ
        assert!(
            segment.samples.len() <= RATE * 21,
            "{}",
            segment.samples.len()
        );
    }
}

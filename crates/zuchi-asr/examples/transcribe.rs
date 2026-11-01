//! WAV を文字起こしする。音声は 100 ms ごとに区切って、実際の取得と同じ形で渡す。
//!
//! ```bash
//! cargo run --release -p zuchi-asr --example transcribe -- 音声.wav
//! ```
//!
//! 発話を標準出力へ、実時間比などを標準エラーへ出す。
//! オプション: `--models <dir>`、`--min-silence <秒>`、`--threshold <値>`、`--min-chars <数>`、
//! `--threads <数>`

use std::env;
use std::error::Error;
use std::time::{Duration, Instant, SystemTime};

use sherpa_onnx::{LinearResampler, Wave};
use zuchi_asr::{
    AsrConfig, ModelPaths, SAMPLE_RATE, SileroDetector, VadConfig, ZipformerTranscriber,
};
use zuchi_core::ports::{SpeechDetector, Transcriber};
use zuchi_core::types::{AudioChannel, AudioFrame, SpeechSegment};

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut wav = None;
    let mut models = "models".to_owned();
    let mut vad_config = VadConfig::default();
    let mut asr_config = AsrConfig::default();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} の値がない"));
        match arg.as_str() {
            "--models" => models = value()?,
            "--min-silence" => vad_config.min_silence_secs = value()?.parse()?,
            "--threshold" => vad_config.threshold = value()?.parse()?,
            "--min-chars" => asr_config.min_chars = value()?.parse()?,
            "--threads" => asr_config.num_threads = value()?.parse()?,
            _ if wav.is_none() => wav = Some(arg),
            _ => return Err(format!("不明な引数: {arg}").into()),
        }
    }
    let wav = wav.ok_or("使い方: transcribe <wav> [--models <dir>] ...")?;

    let wave = Wave::read(&wav).ok_or(format!("WAV を読めなかった: {wav}"))?;
    let samples = to_16k(wave.samples(), wave.sample_rate())?;
    let audio_secs = samples.len() as f64 / f64::from(SAMPLE_RATE);

    let paths = ModelPaths::in_dir(&models);
    let load_started = Instant::now();
    let mut detector = SileroDetector::new(&paths, vad_config)?;
    let mut transcriber = ZipformerTranscriber::new(&paths, asr_config)?;
    let load_secs = load_started.elapsed().as_secs_f64();

    let base = SystemTime::now();
    let started = Instant::now();
    let mut asr_secs = 0.0;
    let mut asr_max_secs: f64 = 0.0;
    let mut segment_count = 0;
    let mut dropped = 0;
    let mut handle = |segment: SpeechSegment| -> Result<(), Box<dyn Error + Send + Sync>> {
        let asr_started = Instant::now();
        let text = transcriber.transcribe(&segment)?;
        let elapsed = asr_started.elapsed().as_secs_f64();
        asr_secs += elapsed;
        asr_max_secs = asr_max_secs.max(elapsed);
        segment_count += 1;
        if text.is_empty() {
            dropped += 1;
        } else {
            let at = |time: SystemTime| time.duration_since(base).unwrap_or_default().as_secs_f64();
            println!(
                "[{:.2}-{:.2}] {text}",
                at(segment.started_at),
                at(segment.ended_at)
            );
        }
        Ok(())
    };

    for (index, chunk) in samples.chunks(SAMPLE_RATE as usize / 10).enumerate() {
        let frame = AudioFrame {
            channel: AudioChannel::System,
            samples: chunk.to_vec(),
            captured_at: base + Duration::from_millis(index as u64 * 100),
        };
        for segment in detector.push(&frame) {
            handle(segment)?;
        }
    }
    for segment in detector.flush() {
        handle(segment)?;
    }

    let total_secs = started.elapsed().as_secs_f64();
    eprintln!(
        "音声 {audio_secs:.1} 秒、読み込み {load_secs:.2} 秒、処理 {total_secs:.2} 秒、\
         実時間比 {:.4}(うち ASR {asr_secs:.2} 秒、1 区間の最大 {asr_max_secs:.2} 秒)、\
         区間 {segment_count}、除去 {dropped}",
        total_secs / audio_secs.max(f64::EPSILON),
    );
    Ok(())
}

fn to_16k(samples: &[f32], rate: i32) -> Result<Vec<f32>, Box<dyn Error + Send + Sync>> {
    if rate == SAMPLE_RATE as i32 {
        return Ok(samples.to_vec());
    }
    let resampler =
        LinearResampler::create(rate, SAMPLE_RATE as i32).ok_or("リサンプラーを作れなかった")?;
    Ok(resampler.resample(samples, true))
}

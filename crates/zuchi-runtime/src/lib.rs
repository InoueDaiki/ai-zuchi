//! 部品をスレッドと channel でつなぐ。Tauri には依存しない。
//!
//! ```text
//! 音声 -> VAD(チャンネルごと) -> 文字起こし(1 つ) -+-> 判定 -> 補足
//! 画面 ------------------------------------------+
//!                                                +-> 記憶(埋め込みで待つので別のスレッド)
//! ```

pub mod reminders;
pub mod secret;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use zuchi_asr::{AsrConfig, ModelPaths, SileroDetector, VadConfig, ZipformerTranscriber};
use zuchi_audio::{MicSource, SystemSource};
use zuchi_core::ports::{
    AudioSource, CaptureHandle, Llm, MemoryStore, Result, ScreenSource, Searcher, SpeechDetector,
    Transcriber,
};
use zuchi_core::types::{
    AudioChannel, AudioFrame, Reaction, ScreenText, SearchHit, SpeechSegment, Suggestion, Utterance,
};
use zuchi_llm::{OllamaConfig, OllamaLlm};
use zuchi_memory::{MemoryConfig, MemoryKey, SqliteMemory};
use zuchi_pipeline::{Engine, EngineConfig, Input};
use zuchi_screen::{PlatformCapturer, PlatformRecognizer, PollingScreenSource, ScreenConfig};
use zuchi_search::{GuardedSearcher, HistorySearcher, SearchLog, SearchRecord, searcher_from_env};

use crate::reminders::Reminders;

/// 埋め込みの再試行の間隔
const EMBED_RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub chat_model: String,
    pub embed_model: String,
    /// 検索語に含まれていたら検索を止める語
    pub blocked_terms: Vec<String>,
    pub listen_screen: bool,
}

/// 部品が動かないときも、残りで動き続けるために記録する。
#[derive(Clone, Debug, PartialEq)]
pub struct SourceFailure {
    pub source: &'static str,
    pub message: String,
}

enum Message {
    Utterance(Utterance),
    Screen(ScreenText),
    Reaction(Suggestion, Reaction),
}

struct SharedSearcher(Arc<dyn Searcher>);

impl Searcher for SharedSearcher {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.0.search(query, limit)
    }
}

struct MemorySearchLog(Arc<SqliteMemory>);

impl SearchLog for MemorySearchLog {
    fn record(&self, record: &SearchRecord) -> Result<()> {
        self.0.record_search(&record.query, record.at)
    }
}

pub struct Runtime {
    config: RuntimeConfig,
    engine_input: Sender<Input>,
    memory_input: Sender<Message>,
    segments: Sender<SpeechSegment>,
    captures: Mutex<Vec<Box<dyn CaptureHandle>>>,
    reminders: Arc<Reminders>,
}

impl Runtime {
    /// 判定と記憶のスレッドを立ち上げる。取得は [`Runtime::listen`] で始める。
    pub fn start(
        config: RuntimeConfig,
        key: MemoryKey,
        suggestions: Sender<Suggestion>,
    ) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        let llm: Arc<dyn Llm> = Arc::new(OllamaLlm::new(OllamaConfig::new(
            config.chat_model.clone(),
            config.embed_model.clone(),
        )));
        let memory = Arc::new(SqliteMemory::open(
            config.data_dir.join("memory.db"),
            &key,
            Arc::clone(&llm),
            MemoryConfig::default(),
        )?);
        let searcher = build_searcher(&config, &memory)?;
        let reminders = Arc::new(Reminders::new(Arc::clone(&memory)));

        let (engine_input, engine_output) = mpsc::channel();
        let store: Arc<dyn MemoryStore> = memory.clone();
        let mut engine = Engine::new(llm, searcher, Some(store), EngineConfig::default());
        thread::Builder::new()
            .name("zuchi-engine".into())
            .spawn(move || engine.run(engine_output, suggestions))?;

        let (memory_input, memory_output) = mpsc::channel();
        thread::Builder::new()
            .name("zuchi-memory".into())
            .spawn(move || run_memory(memory, memory_output))?;

        let paths = ModelPaths::in_dir(&config.models_dir);
        let (segments, segment_output) = mpsc::channel();
        let transcriber = ZipformerTranscriber::new(&paths, AsrConfig::default())?;
        let to_engine = engine_input.clone();
        let to_memory = memory_input.clone();
        thread::Builder::new()
            .name("zuchi-asr".into())
            .spawn(move || run_asr(transcriber, segment_output, to_engine, to_memory))?;

        Ok(Self {
            config,
            engine_input,
            memory_input,
            segments,
            captures: Mutex::new(Vec::new()),
            reminders,
        })
    }

    /// 取得を始める。動かなかった取得元を返す。権限がないなどで全部が失敗することもある。
    pub fn listen(&self) -> Vec<SourceFailure> {
        let mut failures = Vec::new();
        let mut captures = self.captures.lock().unwrap_or_else(|e| e.into_inner());
        if !captures.is_empty() {
            return failures;
        }

        let mic: Box<dyn AudioSource> = Box::new(MicSource::new());
        let system: Box<dyn AudioSource> = Box::new(SystemSource::new());
        for (name, channel, source) in [
            ("マイク", AudioChannel::Mic, mic),
            ("PC の音声", AudioChannel::System, system),
        ] {
            match self.start_audio(channel, source) {
                Ok(handle) => captures.push(handle),
                Err(error) => failures.push(SourceFailure {
                    source: name,
                    message: error.to_string(),
                }),
            }
        }
        if self.config.listen_screen {
            match self.start_screen() {
                Ok(handle) => captures.push(handle),
                Err(error) => failures.push(SourceFailure {
                    source: "画面",
                    message: error.to_string(),
                }),
            }
        }
        failures
    }

    /// 取得を止める。処理中の発話は最後まで流れる。
    pub fn pause(&self) {
        let mut captures = self.captures.lock().unwrap_or_else(|e| e.into_inner());
        for capture in captures.drain(..) {
            capture.stop();
        }
    }

    pub fn is_listening(&self) -> bool {
        !self
            .captures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// 補足への反応を伝える。
    pub fn react(&self, suggestion: Suggestion, reaction: Reaction) {
        let _ = self.engine_input.send(Input::Reaction(reaction));
        let _ = self
            .memory_input
            .send(Message::Reaction(suggestion, reaction));
    }

    /// リマインドの操作口。判定のスレッドには渡さない。
    pub fn reminders(&self) -> Arc<Reminders> {
        Arc::clone(&self.reminders)
    }

    fn start_audio(
        &self,
        channel: AudioChannel,
        mut source: Box<dyn AudioSource>,
    ) -> Result<Box<dyn CaptureHandle>> {
        let paths = ModelPaths::in_dir(&self.config.models_dir);
        let detector = SileroDetector::new(&paths, VadConfig::default())?;
        let (sink, frames) = mpsc::channel();
        let segments = self.segments.clone();
        thread::Builder::new()
            .name(format!("zuchi-vad-{channel:?}"))
            .spawn(move || run_detector(detector, frames, segments))?;
        source.start(sink)
    }

    fn start_screen(&self) -> Result<Box<dyn CaptureHandle>> {
        let mut source = PollingScreenSource::new(ScreenConfig::default(), || {
            Ok((PlatformCapturer::new()?, PlatformRecognizer::new()?))
        });
        let (sink, screens) = mpsc::channel::<ScreenText>();
        let to_engine = self.engine_input.clone();
        let to_memory = self.memory_input.clone();
        thread::Builder::new()
            .name("zuchi-screen-forward".into())
            .spawn(move || {
                for screen in screens {
                    let _ = to_memory.send(Message::Screen(screen.clone()));
                    if to_engine.send(Input::Screen(screen)).is_err() {
                        return;
                    }
                }
            })?;
        source.start(sink)
    }

    /// 文字起こしを飛ばして発話を入れる。再生と確認に使う。
    pub fn inject(&self, utterance: Utterance) {
        let _ = self
            .memory_input
            .send(Message::Utterance(utterance.clone()));
        let _ = self.engine_input.send(Input::Utterance(utterance));
    }
}

fn build_searcher(
    config: &RuntimeConfig,
    memory: &Arc<SqliteMemory>,
) -> Result<Option<Arc<dyn Searcher>>> {
    let Some(inner) = searcher_from_env()? else {
        return Ok(None);
    };
    let inner = SharedSearcher(Arc::from(inner));
    let logged = HistorySearcher::new(inner, MemorySearchLog(Arc::clone(memory)));
    // 止めた語は履歴にも残らないように、除外を外側に置く
    let guarded = GuardedSearcher::new(logged, &config.blocked_terms);
    Ok(Some(Arc::new(guarded)))
}

fn run_detector(
    mut detector: SileroDetector,
    frames: Receiver<AudioFrame>,
    segments: Sender<SpeechSegment>,
) {
    for frame in frames {
        for segment in detector.push(&frame) {
            if segments.send(segment).is_err() {
                return;
            }
        }
    }
    // 止めたときに話している途中の発話を失わない
    for segment in detector.flush() {
        let _ = segments.send(segment);
    }
}

fn run_asr(
    mut transcriber: ZipformerTranscriber,
    segments: Receiver<SpeechSegment>,
    to_engine: Sender<Input>,
    to_memory: Sender<Message>,
) {
    for segment in segments {
        let text = match transcriber.transcribe(&segment) {
            Ok(text) if !text.is_empty() => text,
            Ok(_) => continue,
            Err(_) => {
                eprintln!("文字起こしに失敗した");
                continue;
            }
        };
        let utterance = Utterance {
            channel: segment.channel,
            text,
            started_at: segment.started_at,
            ended_at: segment.ended_at,
        };
        let _ = to_memory.send(Message::Utterance(utterance.clone()));
        if to_engine.send(Input::Utterance(utterance)).is_err() {
            return;
        }
    }
}

fn run_memory(memory: Arc<SqliteMemory>, messages: Receiver<Message>) {
    let mut last_backfill = SystemTime::now();
    for message in messages {
        let result = match message {
            Message::Utterance(utterance) => memory.remember_utterance(&utterance),
            Message::Screen(screen) => memory.remember_screen(&screen),
            Message::Reaction(suggestion, reaction) => {
                memory.remember_reaction(&suggestion, reaction)
            }
        };
        if result.is_err() {
            eprintln!("記憶に失敗した");
        }
        let due = last_backfill
            .elapsed()
            .is_ok_and(|elapsed| elapsed >= EMBED_RETRY);
        if due {
            // Ollama が止まっていた間の埋め込みを埋める
            let _ = memory.embed_pending(32);
            last_backfill = SystemTime::now();
        }
    }
}

//! クレートの間で受け渡す型。

use std::time::{Duration, SystemTime};

/// 期限の指定がないリマインドを、受け付けてから知らせるまでの時間。
pub const DEFAULT_REMINDER_DELAY: Duration = Duration::from_secs(30 * 60);

/// 音声の出どころ。当面はこれを話者の代わりに使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioChannel {
    /// マイク。自分の声として扱う
    Mic,
    /// PC の音声。相手の声として扱う
    System,
}

/// 16 kHz モノラルの音声の断片。ディスクには書かない。
#[derive(Clone, Debug)]
pub struct AudioFrame {
    pub channel: AudioChannel,
    pub samples: Vec<f32>,
    pub captured_at: SystemTime,
}

/// 発話区間。ASR へ渡す前の音声。
#[derive(Clone, Debug)]
pub struct SpeechSegment {
    pub channel: AudioChannel,
    pub samples: Vec<f32>,
    pub started_at: SystemTime,
    pub ended_at: SystemTime,
}

/// 文字起こしされた 1 発話。
#[derive(Clone, Debug, PartialEq)]
pub struct Utterance {
    pub channel: AudioChannel,
    pub text: String,
    pub started_at: SystemTime,
    pub ended_at: SystemTime,
}

/// 画面から読み取った文字。他人が書ける内容なので、指示として扱わない。
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenText {
    pub app: String,
    pub window_title: String,
    pub text: String,
    pub captured_at: SystemTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Skill {
    Calc,
    Search,
    Memory,
    Suggest,
}

/// 補足の根拠。根拠のない検索や記憶の補足は出さない。
#[derive(Clone, Debug, PartialEq)]
pub enum Evidence {
    /// 計算の前提(例: 3 カ月を 91 日とする)
    Assumption(String),
    Web {
        title: String,
        url: String,
    },
    Utterance {
        text: String,
        at: SystemTime,
    },
}

/// 本人の操作の後でだけ実行する操作。
#[derive(Clone, Debug, PartialEq)]
pub enum ProposedAction {
    SetReminder {
        title: String,
        /// `None` なら受け付けてから [`DEFAULT_REMINDER_DELAY`] の後
        due: Option<SystemTime>,
    },
}

/// 画面に出す補足。
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub skill: Skill,
    pub text: String,
    pub evidence: Vec<Evidence>,
    pub action: Option<ProposedAction>,
}

/// 検索の 1 件。
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// 補足への本人の反応。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reaction {
    Helpful,
    Unwanted,
    Ignored,
}

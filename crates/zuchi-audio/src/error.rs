use std::fmt;

/// 取得の開始に失敗した理由。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// 使えるデバイスがない
    NoDevice,
    /// OS が権限を理由に拒否した
    PermissionDenied(String),
    /// デバイスの形式に対応していない
    UnsupportedFormat(String),
    /// 開始が戻らない。権限の確認で待たされている可能性が高い
    StartTimeout,
    /// 音声 API のエラー
    Backend(String),
    /// 補助プログラムが動かない、または異常終了した
    Helper(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => write!(f, "音声デバイスが見つかりません"),
            Self::PermissionDenied(m) => write!(f, "権限がありません: {m}"),
            Self::UnsupportedFormat(m) => write!(f, "対応していない形式です: {m}"),
            Self::StartTimeout => write!(
                f,
                "開始が終わりません。権限のダイアログが出ていないか確認してください"
            ),
            Self::Backend(m) => write!(f, "音声 API のエラー: {m}"),
            Self::Helper(m) => write!(f, "補助プログラムのエラー: {m}"),
        }
    }
}

impl std::error::Error for AudioError {}

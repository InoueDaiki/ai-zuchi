//! 取得と OCR の失敗。文字列に画面の内容を含めない。

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// 画面収録の権限がない。本人が OS の設定で許可するまで直らない
    PermissionDenied,
    /// この OS や環境では動かない(OCR の言語がない、API が古いなど)
    Unsupported(String),
    /// 一時的な失敗かもしれないもの。次の周期でやり直す
    Backend(String),
}

impl CaptureError {
    /// 続けても直らないなら true。取得のスレッドを止める
    pub fn is_fatal(&self) -> bool {
        !matches!(self, Self::Backend(_))
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermissionDenied => write!(
                f,
                "画面収録の権限がありません。OS の設定で本人が許可してください(手順は docs/検証-画面.md)"
            ),
            Self::Unsupported(message) => write!(f, "この環境では使えません: {message}"),
            Self::Backend(message) => write!(f, "画面の取得か OCR に失敗しました: {message}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// `ScreenSource::start` の失敗が権限によるものかを調べる。
pub fn is_permission_denied(error: &zuchi_core::ports::Error) -> bool {
    matches!(
        error.downcast_ref::<CaptureError>(),
        Some(CaptureError::PermissionDenied)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 権限のエラーを見分けられる() {
        let error: zuchi_core::ports::Error = Box::new(CaptureError::PermissionDenied);
        assert!(is_permission_denied(&error));
        let error: zuchi_core::ports::Error = Box::new(CaptureError::Backend("x".into()));
        assert!(!is_permission_denied(&error));
    }

    #[test]
    fn 一時的な失敗だけが続行できる() {
        assert!(CaptureError::PermissionDenied.is_fatal());
        assert!(CaptureError::Unsupported("x".into()).is_fatal());
        assert!(!CaptureError::Backend("x".into()).is_fatal());
    }
}

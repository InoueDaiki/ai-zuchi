//! `zuchi-core` の [`ScreenSource`](zuchi_core::ports::ScreenSource) の実装。
//!
//! 最も手前の窓を一定の間隔で取得し、縮小画像が変わったときだけ OS の OCR で読み、
//! 前に見ていない行だけを流す。画像はメモリの中だけで扱い、ディスクに書かない。
//! ログとエラーには画面の文字を含めない。
//!
//! 変化の検知、差分の抽出、対象外の判定は OS に依存しない。OS の部分は
//! [`WindowCapturer`] と [`TextRecognizer`] の裏に置く。

pub mod change;
pub mod diff;
pub mod error;
pub mod exclude;
pub mod frame;
pub mod ocr;
pub mod pipeline;
pub mod source;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod win;

pub use error::{CaptureError, is_permission_denied};
pub use exclude::{ExcludeList, SkipReason, WindowInfo};
pub use frame::Frame;
pub use ocr::OcrLine;
pub use pipeline::{ScreenConfig, ScreenStats, TextRecognizer, WindowCapturer};
pub use source::PollingScreenSource;

#[cfg(target_os = "macos")]
pub use macos::{MacCapturer as PlatformCapturer, VisionRecognizer as PlatformRecognizer};
#[cfg(windows)]
pub use win::{GraphicsCapturer as PlatformCapturer, WindowsRecognizer as PlatformRecognizer};

/// この OS の `ScreenSource` を作る。権限がなければ `start` が
/// [`CaptureError::PermissionDenied`] を返す。
#[cfg(any(target_os = "macos", windows))]
pub fn platform_source(
    config: ScreenConfig,
) -> PollingScreenSource<PlatformCapturer, PlatformRecognizer> {
    PollingScreenSource::new(config, || {
        Ok((PlatformCapturer::new()?, PlatformRecognizer::new()?))
    })
}

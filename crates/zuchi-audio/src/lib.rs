//! マイクと PC の音声を取得し、16 kHz モノラルの [`AudioFrame`] として流す。音声はディスクに書かない。
//!
//! | 取得元 | macOS | Windows |
//! |---|---|---|
//! | マイク | cpal | cpal |
//! | PC の音声 | Swift の補助プログラム(`native/macos/`)| cpal の WASAPI loopback |
//!
//! macOS の PC の音声に cpal の loopback を使わない理由は `docs/検証-音声取得.md` に書いた。
//!
//! [`AudioFrame`]: zuchi_core::types::AudioFrame

mod cpal_capture;
mod diagnostics;
mod error;
mod frames;
mod resample;
#[cfg(target_os = "macos")]
mod tap;
#[cfg(target_os = "macos")]
mod tap_protocol;

pub use cpal_capture::MicSource;
#[cfg(windows)]
pub use cpal_capture::SystemSource;
pub use diagnostics::{Diagnosis, Level, Permission, diagnose, permission};
pub use error::AudioError;
pub use frames::FRAME_SAMPLES;
pub use resample::TARGET_RATE;
#[cfg(target_os = "macos")]
pub use tap::SystemSource;

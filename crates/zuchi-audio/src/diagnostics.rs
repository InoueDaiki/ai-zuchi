//! 取得できたかどうかの判定。macOS では権限がないと、エラーにならず無音が届くことがある。

use zuchi_core::types::AudioChannel;

/// OS の権限の状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Granted,
    Denied,
    /// まだ尋ねられていない
    Undetermined,
    /// 確認の手段がない
    Unknown,
}

/// 権限の状態を読む。状態は変更せず、ダイアログも出さない。
///
/// macOS では TCC の非公開 API を使うので、補助の情報として扱う。
/// これを理由に取得を止めないこと。
#[cfg(target_os = "macos")]
pub fn permission(channel: AudioChannel) -> Permission {
    let service = match channel {
        AudioChannel::Mic => "mic",
        AudioChannel::System => "system",
    };
    let output = std::process::Command::new(crate::tap::helper_path())
        .args(["--check-permission", service])
        .output();
    match output.as_ref().map(|o| o.stdout.trim_ascii()) {
        Ok(b"granted") => Permission::Granted,
        Ok(b"denied") => Permission::Denied,
        Ok(b"undetermined") => Permission::Undetermined,
        _ => Permission::Unknown,
    }
}

/// 権限の状態を読む。Windows には確認の手段を用意していない。
#[cfg(not(target_os = "macos"))]
pub fn permission(_channel: AudioChannel) -> Permission {
    Permission::Unknown
}

/// 音量の集計。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Level {
    pub frames: usize,
    pub peak: f32,
}

impl Level {
    pub fn add(&mut self, samples: &[f32]) {
        self.frames += 1;
        self.peak = samples.iter().fold(self.peak, |peak, s| peak.max(s.abs()));
    }
}

/// 取得の結果の見立て。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diagnosis {
    /// 音が取れている
    Sound,
    /// 音を鳴らしたのにフレームが届いていない
    NoFrames,
    /// 完全な無音で、権限が拒否されている
    SilentPermissionDenied,
    /// 音を鳴らしたのに完全な無音。権限が原因の可能性が高い
    SilentPermissionSuspected,
    /// 無音、またはフレームがないが、音を鳴らしていない。鳴っていないだけかもしれない
    Silent,
}

/// `played_sound` は、取得の間に確かに音を鳴らしていたか。
pub fn diagnose(level: Level, permission: Permission, played_sound: bool) -> Diagnosis {
    if level.peak > 0.0 {
        Diagnosis::Sound
    } else if permission == Permission::Denied {
        Diagnosis::SilentPermissionDenied
    } else if !played_sound {
        // macOS の tap も、出力が動いていない間はフレームを届けないので、数では判断しない
        Diagnosis::Silent
    } else if level.frames == 0 {
        Diagnosis::NoFrames
    } else {
        Diagnosis::SilentPermissionSuspected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(peak: f32) -> Level {
        Level { frames: 10, peak }
    }

    #[test]
    fn 音があれば権限の状態によらず取れている() {
        for status in [Permission::Unknown, Permission::Denied] {
            assert_eq!(diagnose(level(0.1), status, false), Diagnosis::Sound);
        }
    }

    #[test]
    fn 拒否されていて無音なら権限が原因() {
        assert_eq!(
            diagnose(level(0.0), Permission::Denied, false),
            Diagnosis::SilentPermissionDenied
        );
    }

    #[test]
    fn 鳴らしたのに無音なら権限を疑う() {
        assert_eq!(
            diagnose(level(0.0), Permission::Undetermined, true),
            Diagnosis::SilentPermissionSuspected
        );
    }

    #[test]
    fn 鳴らしていない無音は決めつけない() {
        assert_eq!(
            diagnose(level(0.0), Permission::Granted, false),
            Diagnosis::Silent
        );
    }

    #[test]
    fn フレームがなければそう報告する() {
        assert_eq!(
            diagnose(Level::default(), Permission::Granted, true),
            Diagnosis::NoFrames
        );
    }

    #[test]
    fn ピークは絶対値で集計する() {
        let mut level = Level::default();
        level.add(&[0.1, -0.5]);
        level.add(&[0.2]);
        assert_eq!(
            level,
            Level {
                frames: 2,
                peak: 0.5
            }
        );
    }
}

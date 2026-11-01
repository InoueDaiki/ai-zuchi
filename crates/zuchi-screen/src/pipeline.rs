//! 1 周期の処理。OS の部分は trait の裏に置き、Fake でテストする。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use zuchi_core::types::ScreenText;

use crate::change::{ChangeConfig, ChangeDecision, ChangeDetector};
use crate::diff::TextDiffer;
use crate::error::CaptureError;
use crate::exclude::{ExcludeList, WindowInfo};
use crate::frame::Frame;
use crate::ocr::OcrLine;

/// 画面の取得の設定。
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenConfig {
    /// 画面を見に行く間隔
    pub interval: Duration,
    pub change: ChangeConfig,
    pub exclude: ExcludeList,
    /// OCR の確信度がこれ未満の行は捨てる
    pub min_confidence: f32,
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(1500),
            change: ChangeConfig::default(),
            exclude: ExcludeList::default(),
            min_confidence: 0.3,
        }
    }
}

/// 窓の一覧と画像の取得。OS ごとに実装する。
pub trait WindowCapturer {
    /// 通常の階層の窓を、手前から順に返す。
    fn list_windows(&mut self) -> Result<Vec<WindowInfo>, CaptureError>;
    /// その窓だけを取得する。他の窓が重なっていても写らない。
    fn capture(&mut self, window: &WindowInfo) -> Result<Frame, CaptureError>;
}

/// 画像の文字を読む。OS ごとに実装する。
pub trait TextRecognizer {
    fn recognize(&mut self, frame: &Frame) -> Result<Vec<OcrLine>, CaptureError>;
}

/// 動作の回数。内容は持たないので、そのまま表示や記録ができる。
#[derive(Debug, Default)]
pub struct ScreenStats {
    pub ticks: AtomicU64,
    /// 対象外などで読まなかった回数
    pub skipped: AtomicU64,
    /// 窓を取得した回数
    pub captures: AtomicU64,
    /// OCR の回数
    pub ocr_runs: AtomicU64,
    /// 流した [`ScreenText`] の数
    pub emitted: AtomicU64,
    /// 取得か OCR に失敗した回数
    pub errors: AtomicU64,
}

/// 周期の結果を決める処理。
pub struct Pipeline<C, R> {
    capturer: C,
    recognizer: R,
    exclude: ExcludeList,
    min_confidence: f32,
    detector: ChangeDetector,
    differ: TextDiffer,
    current_window: Option<u64>,
    stats: Arc<ScreenStats>,
}

impl<C: WindowCapturer, R: TextRecognizer> Pipeline<C, R> {
    pub fn new(capturer: C, recognizer: R, config: &ScreenConfig, stats: Arc<ScreenStats>) -> Self {
        Self {
            capturer,
            recognizer,
            exclude: config.exclude.clone(),
            min_confidence: config.min_confidence,
            detector: ChangeDetector::new(config.change.clone()),
            differ: TextDiffer::default(),
            current_window: None,
            stats,
        }
    }

    /// 読む窓を選ぶ。失敗や対象外では `None`。読まなかった窓の状態は残さない。
    fn pick_window(&mut self) -> Result<Option<WindowInfo>, CaptureError> {
        let windows = self.capturer.list_windows()?;
        match self.exclude.pick(&windows) {
            Ok(window) => Ok(Some(window.clone())),
            Err(reason) => {
                log::debug!("画面を読みません: {reason:?}");
                self.stats.skipped.fetch_add(1, Ordering::Relaxed);
                self.current_window = None;
                self.detector.reset();
                Ok(None)
            }
        }
    }

    /// 1 周期ぶん処理する。新しい文字があれば返す。
    pub fn tick(&mut self) -> Result<Option<ScreenText>, CaptureError> {
        self.stats.ticks.fetch_add(1, Ordering::Relaxed);
        let Some(window) = self.pick_window()? else {
            return Ok(None);
        };
        if self.current_window != Some(window.id) {
            self.detector.reset();
            self.current_window = Some(window.id);
        }

        let frame = self.capturer.capture(&window)?;
        self.stats.captures.fetch_add(1, Ordering::Relaxed);
        let thumbnail = self.detector.thumbnail_of(&frame);
        if self.detector.observe(thumbnail) != ChangeDecision::Read {
            return Ok(None);
        }

        // 取得の間にタブが銀行へ替わったかもしれない。読む前に確かめ直す
        if self.pick_window()?.as_ref() != Some(&window) {
            self.detector.reset();
            return Ok(None);
        }

        let captured_at = SystemTime::now();
        self.stats.ocr_runs.fetch_add(1, Ordering::Relaxed);
        let lines = self.recognizer.recognize(&frame)?;
        drop(frame);

        let lines: Vec<String> = lines
            .into_iter()
            .filter(|line| line.confidence >= self.min_confidence)
            .map(|line| line.text)
            .collect();
        let fresh = self.differ.take_new(window.id, &lines);
        if fresh.is_empty() {
            return Ok(None);
        }
        self.stats.emitted.fetch_add(1, Ordering::Relaxed);
        Ok(Some(ScreenText {
            app: window.app_name,
            window_title: window.title.unwrap_or_default(),
            text: fresh.join("\n"),
            captured_at,
        }))
    }
}

#[cfg(test)]
pub(crate) mod fakes {
    use std::collections::VecDeque;

    use super::*;

    pub fn window(id: u64, app_id: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id,
            pid: u32::MAX,
            app_id: app_id.into(),
            app_name: "アプリ".into(),
            title: Some(title.into()),
        }
    }

    /// 全面が同じ明るさの画像。明るさで画面の違いを表す。
    pub fn frame(level: u8) -> Frame {
        Frame {
            width: 320,
            height: 180,
            stride: 320 * 4,
            bgra: vec![level; 320 * 180 * 4],
        }
    }

    /// 台本どおりに窓と画像を返す。尽きたら最後のものを返し続ける。
    pub struct FakeCapturer {
        pub windows: VecDeque<Result<Vec<WindowInfo>, CaptureError>>,
        pub frames: VecDeque<Frame>,
    }

    impl WindowCapturer for FakeCapturer {
        fn list_windows(&mut self) -> Result<Vec<WindowInfo>, CaptureError> {
            if self.windows.len() > 1 {
                self.windows.pop_front().unwrap()
            } else {
                self.windows.front().unwrap().clone()
            }
        }

        fn capture(&mut self, _window: &WindowInfo) -> Result<Frame, CaptureError> {
            if self.frames.len() > 1 {
                Ok(self.frames.pop_front().unwrap())
            } else {
                Ok(self.frames.front().unwrap().clone())
            }
        }
    }

    /// 画像の明るさに応じた行を返す。
    pub struct FakeRecognizer;

    impl TextRecognizer for FakeRecognizer {
        fn recognize(&mut self, frame: &Frame) -> Result<Vec<OcrLine>, CaptureError> {
            let line = |text: String, confidence| OcrLine {
                text,
                confidence,
                x: 0.0,
                y: 0.0,
                height: 0.1,
            };
            Ok(vec![
                line("共通のヘッダー".into(), 0.9),
                line(format!("本文その{}", frame.bgra[0]), 0.9),
                line("自信のない行".into(), 0.1),
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::fakes::*;
    use super::*;

    fn pipeline(
        windows: Vec<Result<Vec<WindowInfo>, CaptureError>>,
        frames: Vec<Frame>,
    ) -> Pipeline<FakeCapturer, FakeRecognizer> {
        Pipeline::new(
            FakeCapturer {
                windows: VecDeque::from(windows),
                frames: VecDeque::from(frames),
            },
            FakeRecognizer,
            &ScreenConfig::default(),
            Arc::default(),
        )
    }

    fn slack() -> WindowInfo {
        window(1, "com.tinyspeck.slackmacgap", "Slack")
    }

    #[test]
    fn 初回は窓の文字を流す() {
        let mut p = pipeline(vec![Ok(vec![slack()])], vec![frame(10)]);
        let text = p.tick().unwrap().unwrap();
        assert_eq!(text.text, "共通のヘッダー\n本文その10");
        assert_eq!(text.app, "アプリ");
        assert_eq!(text.window_title, "Slack");
    }

    #[test]
    fn 変わらなければ読み直さない() {
        let mut p = pipeline(vec![Ok(vec![slack()])], vec![frame(10)]);
        p.tick().unwrap();
        assert_eq!(p.tick().unwrap(), None);
        assert_eq!(p.stats.ocr_runs.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn 変わったら差分だけを流す() {
        let mut p = pipeline(vec![Ok(vec![slack()])], vec![frame(10), frame(200)]);
        p.tick().unwrap();
        assert_eq!(p.tick().unwrap(), None); // 落ち着くのを待つ
        assert_eq!(p.tick().unwrap().unwrap().text, "本文その200");
    }

    #[test]
    fn 対象外の窓は取得もしない() {
        let mut p = pipeline(
            vec![Ok(vec![window(2, "com.1password.1password", "金庫")])],
            vec![frame(10)],
        );
        assert_eq!(p.tick().unwrap(), None);
        assert_eq!(p.stats.captures.load(Ordering::Relaxed), 0);
        assert_eq!(p.stats.skipped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn 取得の間に題名が対象外へ替わったら読まない() {
        let bank = window(1, "com.apple.Safari", "○○銀行");
        let mut p = pipeline(vec![Ok(vec![slack()]), Ok(vec![bank])], vec![frame(10)]);
        assert_eq!(p.tick().unwrap(), None);
        assert_eq!(p.stats.captures.load(Ordering::Relaxed), 1);
        assert_eq!(p.stats.ocr_runs.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn 取得の間に手前の窓が替わったら読まない() {
        let other = window(3, "com.apple.Notes", "メモ");
        let mut p = pipeline(vec![Ok(vec![slack()]), Ok(vec![other])], vec![frame(10)]);
        assert_eq!(p.tick().unwrap(), None);
        assert_eq!(p.stats.ocr_runs.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn 窓を替えて戻っても同じ文字は流さない() {
        let notes = window(3, "com.apple.Notes", "メモ");
        let mut p = pipeline(
            vec![
                Ok(vec![slack()]),
                Ok(vec![slack()]),
                Ok(vec![notes.clone()]),
                Ok(vec![notes]),
                Ok(vec![slack()]),
            ],
            vec![frame(10)],
        );
        assert!(p.tick().unwrap().is_some());
        assert!(p.tick().unwrap().is_some()); // 別の窓は別に覚える
        assert_eq!(p.tick().unwrap(), None);
        assert_eq!(p.stats.ocr_runs.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn 権限のエラーをそのまま返す() {
        let mut p = pipeline(vec![Err(CaptureError::PermissionDenied)], vec![frame(10)]);
        assert_eq!(p.tick(), Err(CaptureError::PermissionDenied));
    }
}

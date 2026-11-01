//! 周期の処理をスレッドで回し、`ScreenSource` にする。

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use zuchi_core::ports::{CaptureHandle, Result, ScreenSource};
use zuchi_core::types::ScreenText;

use crate::error::CaptureError;
use crate::pipeline::{Pipeline, ScreenConfig, ScreenStats, TextRecognizer, WindowCapturer};

/// OS の部品を作る関数。部品はスレッドをまたげないことがあるので、取得のスレッドの中で作る。
type Factory<C, R> = Arc<dyn Fn() -> std::result::Result<(C, R), CaptureError> + Send + Sync>;

/// 一定の間隔で画面を見に行く `ScreenSource`。
pub struct PollingScreenSource<C, R> {
    config: ScreenConfig,
    factory: Factory<C, R>,
    stats: Arc<ScreenStats>,
    fatal: Arc<Mutex<Option<CaptureError>>>,
}

impl<C, R> PollingScreenSource<C, R> {
    pub fn new(
        config: ScreenConfig,
        factory: impl Fn() -> std::result::Result<(C, R), CaptureError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            config,
            factory: Arc::new(factory),
            stats: Arc::default(),
            fatal: Arc::default(),
        }
    }

    /// 動作の回数。内容は含まない。
    pub fn stats(&self) -> Arc<ScreenStats> {
        Arc::clone(&self.stats)
    }

    /// 取得が止まった理由。権限を途中で外された場合などに入る。
    pub fn fatal_error(&self) -> Option<CaptureError> {
        self.fatal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl<C, R> ScreenSource for PollingScreenSource<C, R>
where
    C: WindowCapturer + 'static,
    R: TextRecognizer + 'static,
{
    fn start(&mut self, sink: Sender<ScreenText>) -> Result<Box<dyn CaptureHandle>> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let factory = Arc::clone(&self.factory);
        let config = self.config.clone();
        let stats = Arc::clone(&self.stats);
        let fatal = Arc::clone(&self.fatal);

        let thread = std::thread::Builder::new()
            .name("zuchi-screen".into())
            .spawn(move || {
                let (capturer, recognizer) = match factory() {
                    Ok(parts) => {
                        let _ = ready_tx.send(Ok(()));
                        parts
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let mut pipeline = Pipeline::new(capturer, recognizer, &config, stats.clone());
                loop {
                    match pipeline.tick() {
                        Ok(Some(text)) => {
                            if sink.send(text).is_err() {
                                return;
                            }
                        }
                        Ok(None) => {}
                        Err(error) if error.is_fatal() => {
                            log::error!("画面の取得を止めます: {error}");
                            *fatal.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
                            return;
                        }
                        Err(error) => {
                            stats.errors.fetch_add(1, Ordering::Relaxed);
                            log::warn!("{error}");
                        }
                    }
                    match stop_rx.recv_timeout(config.interval) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Box::new(Handle {
                stop: Some(stop_tx),
                thread: Some(thread),
            })),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error.into())
            }
            Err(_) => {
                let _ = thread.join();
                Err(CaptureError::Backend("取得のスレッドが起動しませんでした".into()).into())
            }
        }
    }
}

struct Handle {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureHandle for Handle {
    fn stop(self: Box<Self>) {}
}

/// `stop` でも drop でも、ここで止まる。
impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use super::*;
    use crate::error::is_permission_denied;
    use crate::pipeline::fakes::*;

    fn config() -> ScreenConfig {
        ScreenConfig {
            interval: Duration::from_millis(1),
            ..Default::default()
        }
    }

    fn capturer(
        windows: std::result::Result<Vec<crate::exclude::WindowInfo>, CaptureError>,
    ) -> FakeCapturer {
        FakeCapturer {
            windows: VecDeque::from([windows]),
            frames: VecDeque::from([frame(10)]),
        }
    }

    #[test]
    fn 文字を流しstopで止まる() {
        let mut source = PollingScreenSource::new(config(), || {
            Ok((capturer(Ok(vec![window(1, "app", "題名")])), FakeRecognizer))
        });
        let (tx, rx) = mpsc::channel();
        let handle = source.start(tx).unwrap();
        let text = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(text.text.contains("本文その10"));
        handle.stop();
        assert!(rx.recv().is_err(), "止めたら送信側が閉じる");
    }

    #[test]
    fn dropでも止まる() {
        let mut source = PollingScreenSource::new(config(), || {
            Ok((capturer(Ok(vec![window(1, "app", "題名")])), FakeRecognizer))
        });
        let (tx, rx) = mpsc::channel();
        drop(source.start(tx).unwrap());
        while rx.try_recv().is_ok() {}
        assert!(rx.recv().is_err());
    }

    #[test]
    fn 起動時の権限エラーはstartが返す() {
        let mut source = PollingScreenSource::<FakeCapturer, FakeRecognizer>::new(config(), || {
            Err(CaptureError::PermissionDenied)
        });
        let (tx, _rx) = mpsc::channel();
        let error = source.start(tx).err().unwrap();
        assert!(is_permission_denied(&error));
    }

    #[test]
    fn 途中で権限を失うと止まって理由が残る() {
        let mut source = PollingScreenSource::new(config(), || {
            Ok((
                capturer(Err(CaptureError::PermissionDenied)),
                FakeRecognizer,
            ))
        });
        let (tx, rx) = mpsc::channel();
        let _handle = source.start(tx).unwrap();
        assert!(rx.recv().is_err());
        assert_eq!(source.fatal_error(), Some(CaptureError::PermissionDenied));
    }

    #[test]
    fn 一時的な失敗では止まらない() {
        let mut source = PollingScreenSource::new(config(), || {
            let mut fake = capturer(Ok(vec![window(1, "app", "題名")]));
            fake.windows
                .push_front(Err(CaptureError::Backend("x".into())));
            Ok((fake, FakeRecognizer))
        });
        let stats = source.stats();
        let (tx, rx) = mpsc::channel();
        let _handle = source.start(tx).unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        assert_eq!(stats.errors.load(Ordering::Relaxed), 1);
    }
}

//! macOS の PC の音声。Swift の補助プログラムを子プロセスとして動かし、stdout から PCM を読む。

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use zuchi_core::ports::{AudioSource, CaptureHandle, Result};
use zuchi_core::types::{AudioChannel, AudioFrame};

use crate::error::AudioError;
use crate::frames::FrameBuilder;
use crate::tap_protocol::read_packet;

/// 開始を待つ時間。初回は権限のダイアログで止まることがある
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// stdin を閉じてから、強制終了するまでの猶予。補助プログラムの片付けの上限(2 秒)より長くする
const STOP_GRACE: Duration = Duration::from_secs(3);

const EXIT_UNSUPPORTED_FORMAT: i32 = 65;

/// ビルド時に作った補助プログラムの場所。
pub(crate) fn helper_path() -> PathBuf {
    PathBuf::from(env!("ZUCHI_AUDIO_TAP_PATH"))
}

/// PC の音声全体を取得する。
pub struct SystemSource {
    helper: PathBuf,
}

impl Default for SystemSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemSource {
    pub fn new() -> Self {
        Self::with_helper(helper_path())
    }

    /// 補助プログラムの場所を指定する。アプリに同梱したものを使うときに渡す。
    pub fn with_helper(helper: impl Into<PathBuf>) -> Self {
        Self {
            helper: helper.into(),
        }
    }
}

impl AudioSource for SystemSource {
    fn start(&mut self, sink: Sender<AudioFrame>) -> Result<Box<dyn CaptureHandle>> {
        Ok(Box::new(start_helper(&self.helper, sink, START_TIMEOUT)?))
    }
}

fn start_helper(
    helper: &Path,
    sink: Sender<AudioFrame>,
    timeout: Duration,
) -> std::result::Result<TapHandle, AudioError> {
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AudioError::Helper(format!("{} を起動できません: {e}", helper.display())))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout は piped");
    let stderr = child.stderr.take().expect("stderr は piped");

    let (ready_tx, ready_rx) = channel();
    let reader = thread::spawn(move || read_loop(stdout, sink, ready_tx));
    let errors = thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut text);
        text
    });

    let mut handle = TapHandle {
        child: Some(child),
        stdin,
        reader: Some(reader),
    };
    match ready_rx.recv_timeout(timeout) {
        Ok(()) => Ok(handle),
        Err(RecvTimeoutError::Timeout) => {
            handle.shutdown();
            Err(AudioError::StartTimeout)
        }
        Err(RecvTimeoutError::Disconnected) => {
            let status = handle.shutdown();
            let message = errors.join().unwrap_or_default();
            Err(start_error(status, message.trim()))
        }
    }
}

fn start_error(status: Option<ExitStatus>, message: &str) -> AudioError {
    let code = status.and_then(|s| s.code());
    if code == Some(EXIT_UNSUPPORTED_FORMAT) {
        return AudioError::UnsupportedFormat(message.to_owned());
    }
    AudioError::Helper(format!("終了コード {code:?}: {message}"))
}

/// stdout が閉じるか、受け手がいなくなるまで読む。
///
/// 最初のパケットで `ready` に知らせる。開始前に終わると `ready` が閉じ、失敗として伝わる。
fn read_loop(stdout: impl Read, sink: Sender<AudioFrame>, ready: Sender<()>) {
    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    let mut builder = FrameBuilder::new(AudioChannel::System, sink);
    let mut ready = Some(ready);
    while let Ok(Some(packet)) = read_packet(&mut reader) {
        if let Some(ready) = ready.take() {
            let _ = ready.send(());
        }
        if builder
            .push(&packet.samples, packet.sample_rate, packet.channels)
            .is_err()
        {
            break;
        }
    }
    // ここで stdout を閉じる。書き込みが EPIPE になり、補助プログラムも終わる
}

struct TapHandle {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
}

impl TapHandle {
    /// 子プロセスと読み取りスレッドの終了を待って返る。何度呼んでもよい。
    fn shutdown(&mut self) -> Option<ExitStatus> {
        // stdin を閉じるのが、補助プログラムへの終了の合図
        drop(self.stdin.take());
        let status = self.child.take().and_then(|mut child| {
            let deadline = Instant::now() + STOP_GRACE;
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(status)) => return Some(status),
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
            let _ = child.kill();
            child.wait().ok()
        });
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        status
    }
}

impl CaptureHandle for TapHandle {
    fn stop(mut self: Box<Self>) {
        self.shutdown();
    }
}

impl Drop for TapHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    /// 書き込み中のスクリプトを別のテストの fork が開いたままにすると、起動が ETXTBSY で失敗する。
    /// スクリプトを使うテストは順に動かす
    static SERIAL: Mutex<()> = Mutex::new(());

    /// 開始のパケット(48 kHz、2 チャンネル、本体 0 バイト)を出す sh のコマンド
    const READY: &str = r"printf 'ZTAP\200\273\000\000\002\000\000\000\000\000\000\000'";

    struct Fake {
        dir: PathBuf,
        _serial: MutexGuard<'static, ()>,
    }

    impl Fake {
        fn new(name: &str, body: &str) -> Self {
            let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir()
                .join(format!("zuchi-audio-test-{}-{name}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            let script = dir.join("helper.sh");
            let text = format!("#!/bin/sh\necho $$ > '{}/pid'\n{body}\n", dir.display());
            fs::write(&script, text).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            Self {
                dir,
                _serial: serial,
            }
        }

        fn script(&self) -> PathBuf {
            self.dir.join("helper.sh")
        }

        fn is_running(&self) -> bool {
            let pid = fs::read_to_string(self.dir.join("pid")).unwrap();
            Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn stop_は_stdin_を閉じて子プロセスを止める() {
        let fake = Fake::new("graceful", &format!("{READY}\nexec cat > /dev/null"));
        let (tx, _rx) = channel();
        let handle = start_helper(&fake.script(), tx, Duration::from_secs(5)).unwrap();
        assert!(fake.is_running());
        let started = Instant::now();
        Box::new(handle).stop();
        assert!(started.elapsed() < STOP_GRACE);
        assert!(!fake.is_running());
    }

    #[test]
    fn stdin_を無視する子プロセスは強制終了する() {
        let fake = Fake::new("stubborn", &format!("{READY}\nexec sleep 60"));
        let (tx, _rx) = channel();
        let handle = start_helper(&fake.script(), tx, Duration::from_secs(5)).unwrap();
        Box::new(handle).stop();
        assert!(!fake.is_running());
    }

    #[test]
    fn drop_でも止まる() {
        let fake = Fake::new("dropped", &format!("{READY}\nexec sleep 60"));
        let (tx, _rx) = channel();
        drop(start_helper(&fake.script(), tx, Duration::from_secs(5)).unwrap());
        assert!(!fake.is_running());
    }

    #[test]
    fn 開始の失敗は理由つきのエラーになる() {
        let fake = Fake::new("failing", "echo 'tap の作成に失敗' >&2\nexit 69");
        let (tx, _rx) = channel();
        let error = start_helper(&fake.script(), tx, Duration::from_secs(5)).err();
        match error {
            Some(AudioError::Helper(message)) => {
                assert!(message.contains("69") && message.contains("tap の作成に失敗"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 開始しなければ時間切れで止める() {
        let fake = Fake::new("silent", "exec sleep 60");
        let (tx, _rx) = channel();
        let error = start_helper(&fake.script(), tx, Duration::from_secs(1)).err();
        assert_eq!(error, Some(AudioError::StartTimeout));
        assert!(!fake.is_running());
    }

    #[test]
    fn 補助プログラムがないとエラーになる() {
        let (tx, _rx) = channel();
        let error = start_helper(Path::new("/nonexistent"), tx, Duration::from_secs(1)).err();
        assert!(matches!(error, Some(AudioError::Helper(_))));
    }

    #[test]
    fn 本物の補助プログラムが権限の状態を答える() {
        // 取得はしない。権限のダイアログも出ない
        let output = Command::new(helper_path())
            .args(["--check-permission", "system"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(["granted", "denied", "undetermined", "unknown"].contains(&text.trim()));
    }
}

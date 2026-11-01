//! cpal による取得。マイクと、Windows の PC の音声(WASAPI loopback)で使う。

use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use zuchi_core::ports::{AudioSource, CaptureHandle, Result};
use zuchi_core::types::{AudioChannel, AudioFrame};

use crate::error::AudioError;
use crate::frames::FrameBuilder;

/// 開始を待つ時間。macOS では権限の確認が済むまで、ストリームの作成が戻らないことがある
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// 既定の入力デバイスから取得する。
#[derive(Default)]
pub struct MicSource;

impl MicSource {
    pub fn new() -> Self {
        Self
    }
}

impl AudioSource for MicSource {
    fn start(&mut self, sink: Sender<AudioFrame>) -> Result<Box<dyn CaptureHandle>> {
        start_capture(AudioChannel::Mic, sink, START_TIMEOUT, || {
            let device = cpal::default_host()
                .default_input_device()
                .ok_or(AudioError::NoDevice)?;
            let config = device.default_input_config().map_err(to_audio_error)?;
            Ok((device, config))
        })
    }
}

/// 既定の出力デバイスの音声を WASAPI loopback で取得する。
///
/// cpal は、出力デバイスから入力のストリームを作ると loopback にする。
/// 何も鳴っていない間は、WASAPI がデータを届けないのでフレームも流れない。
#[cfg(windows)]
#[derive(Default)]
pub struct SystemSource;

#[cfg(windows)]
impl SystemSource {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(windows)]
impl AudioSource for SystemSource {
    fn start(&mut self, sink: Sender<AudioFrame>) -> Result<Box<dyn CaptureHandle>> {
        start_capture(AudioChannel::System, sink, START_TIMEOUT, || {
            let device = cpal::default_host()
                .default_output_device()
                .ok_or(AudioError::NoDevice)?;
            let config = device.default_output_config().map_err(to_audio_error)?;
            Ok((device, config))
        })
    }
}

type Selection = std::result::Result<(Device, cpal::SupportedStreamConfig), AudioError>;

/// `Stream` はプラットフォームによって `Send` でないので、作成から破棄まで専用のスレッドで持つ。
fn start_capture(
    channel_kind: AudioChannel,
    sink: Sender<AudioFrame>,
    timeout: Duration,
    select: impl FnOnce() -> Selection + Send + 'static,
) -> Result<Box<dyn CaptureHandle>> {
    let (stop_tx, stop_rx) = channel::<()>();
    let (ready_tx, ready_rx) = channel();
    let stop_signal = stop_tx.clone();

    let thread = thread::Builder::new()
        .name("zuchi-audio-capture".into())
        .spawn(move || {
            let stream = select().and_then(|(device, config)| {
                let builder = FrameBuilder::new(channel_kind, sink);
                let stream = build_stream(&device, &config, builder, stop_signal)?;
                stream.play().map_err(to_audio_error)?;
                Ok(stream)
            });
            match stream {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    // 止める合図か、受け手の消失か、続行できないエラーまで待つ
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
        })?;

    match ready_rx.recv_timeout(timeout) {
        Ok(Ok(())) => Ok(Box::new(CpalHandle {
            stop: Some(stop_tx),
            thread: Some(thread),
        })),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error.into())
        }
        // OS の呼び出しの中で止まっているので join できない。
        // 戻ってきたら `stop_rx` が閉じているので、スレッドは取得を始めずに終わる
        Err(RecvTimeoutError::Timeout) => Err(AudioError::StartTimeout.into()),
        Err(RecvTimeoutError::Disconnected) => {
            let _ = thread.join();
            Err(AudioError::Backend("取得のスレッドが異常終了しました".into()).into())
        }
    }
}

fn build_stream(
    device: &Device,
    config: &cpal::SupportedStreamConfig,
    builder: FrameBuilder,
    stop: Sender<()>,
) -> std::result::Result<Stream, AudioError> {
    let format = config.sample_format();
    let config = config.config();
    match format {
        SampleFormat::F32 => typed_stream::<f32>(device, config, builder, stop),
        SampleFormat::F64 => typed_stream::<f64>(device, config, builder, stop),
        SampleFormat::I16 => typed_stream::<i16>(device, config, builder, stop),
        SampleFormat::I32 => typed_stream::<i32>(device, config, builder, stop),
        SampleFormat::U16 => typed_stream::<u16>(device, config, builder, stop),
        other => Err(AudioError::UnsupportedFormat(format!("{other:?}"))),
    }
}

fn typed_stream<T>(
    device: &Device,
    config: StreamConfig,
    mut builder: FrameBuilder,
    stop: Sender<()>,
) -> std::result::Result<Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let rate = config.sample_rate;
    let channels = usize::from(config.channels);
    let stop_on_error = stop.clone();
    let mut converted: Vec<f32> = Vec::new();

    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                converted.clear();
                converted.extend(data.iter().map(|&s| s.to_sample::<f32>()));
                if builder.push(&converted, rate, channels).is_err() {
                    let _ = stop.send(());
                }
            },
            move |error| {
                if is_fatal(error.kind()) {
                    eprintln!("音声の取得を止めます: {error}");
                    let _ = stop_on_error.send(());
                }
            },
            None,
        )
        .map_err(to_audio_error)
}

/// 取得を続けられないエラーか。止めると `sink` が閉じ、受け手に伝わる。
fn is_fatal(kind: ErrorKind) -> bool {
    !matches!(
        kind,
        ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
    )
}

fn to_audio_error(error: cpal::Error) -> AudioError {
    match error.kind() {
        ErrorKind::PermissionDenied => AudioError::PermissionDenied(error.to_string()),
        ErrorKind::DeviceNotAvailable => AudioError::NoDevice,
        ErrorKind::UnsupportedConfig => AudioError::UnsupportedFormat(error.to_string()),
        _ => AudioError::Backend(error.to_string()),
    }
}

struct CpalHandle {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl CpalHandle {
    fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl CaptureHandle for CpalHandle {
    fn stop(mut self: Box<Self>) {
        self.shutdown();
    }
}

impl Drop for CpalHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 一時的なエラーでは止めない() {
        assert!(!is_fatal(ErrorKind::Xrun));
        assert!(!is_fatal(ErrorKind::DeviceChanged));
        assert!(is_fatal(ErrorKind::DeviceNotAvailable));
        assert!(is_fatal(ErrorKind::StreamInvalidated));
    }

    #[test]
    fn 開始が戻らなければ時間切れで返す() {
        let (tx, _rx) = channel();
        let error = start_capture(AudioChannel::Mic, tx, Duration::from_millis(10), || {
            thread::sleep(Duration::from_millis(200));
            Err(AudioError::NoDevice)
        })
        .err()
        .unwrap();
        assert_eq!(
            error.downcast_ref::<AudioError>(),
            Some(&AudioError::StartTimeout)
        );
    }

    #[test]
    fn 権限のエラーを区別する() {
        let error = cpal::Error::from(ErrorKind::PermissionDenied);
        assert!(matches!(
            to_audio_error(error),
            AudioError::PermissionDenied(_)
        ));
    }
}

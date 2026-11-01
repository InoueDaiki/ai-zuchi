//! macOS の実装。ScreenCaptureKit で窓を取得し、Vision で読む。
//!
//! 同じ処理の中で動くので、画像を別の処理へ渡さずに済む。Swift の補助プログラムを
//! 使わない理由は docs/検証-画面.md に書いた。
//!
//! 権限がないときは ScreenCaptureKit を呼ばない。呼ぶと OS の確認が出るので、
//! 確認を出すのは本人が [`request_permission`] を呼んだときだけにする。

use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFArray, CFData, CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGContext,
    CGDataProvider, CGImage, CGMainDisplayID, CGPreflightScreenCaptureAccess,
    CGRequestScreenCaptureAccess, CGWindowListCreate, CGWindowListOption,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

use crate::error::CaptureError;
use crate::exclude::WindowInfo;
use crate::frame::Frame;
use crate::ocr::{OcrLine, sort_reading_order};
use crate::pipeline::{TextRecognizer, WindowCapturer};

/// 完了の通知を待つ上限。超えたら一時的な失敗として次の周期に任せる。
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5);
/// 小さな窓(通知、メニューバーのアプリなど)は読まない。
const MIN_WINDOW_POINTS: f64 = 120.0;
/// `SCStreamErrorUserDeclined`
const SC_ERROR_USER_DECLINED: isize = -3801;
/// 32 ビットのリトルエンディアンで、アルファが先頭の乗算済み。メモリ上は BGRA
const BGRA_BITMAP_INFO: u32 = 2 | (2 << 12);

/// 画面収録の権限があるかを調べる。確認の画面は出さない。
pub fn has_permission() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// OS の確認を出す。本人が明示的に求めたときだけ呼ぶ。
pub fn request_permission() -> bool {
    CGRequestScreenCaptureAccess()
}

/// 完了の通知から結果を受け取るための入れ物。
struct Delivered<T>(Result<T, CaptureError>);
// SAFETY: 中身は ScreenCaptureKit の不変なオブジェクトか CGImage で、受け渡しの後は受け手だけが使う
unsafe impl<T> Send for Delivered<T> {}

fn describe(error: &NSError) -> CaptureError {
    if error.code() == SC_ERROR_USER_DECLINED {
        return CaptureError::PermissionDenied;
    }
    CaptureError::Backend(format!("{} ({})", error.domain(), error.code()))
}

/// 完了の通知を待つ。
fn wait<T>(rx: &mpsc::Receiver<Delivered<T>>) -> Result<T, CaptureError> {
    rx.recv_timeout(CALLBACK_TIMEOUT)
        .map_err(|_| CaptureError::Backend("完了の通知が来ませんでした".into()))?
        .0
}

fn deliver<T, U>(
    tx: &Sender<Delivered<U>>,
    item: *mut T,
    error: *mut NSError,
    wrap: impl FnOnce(NonNull<T>) -> U,
) {
    let result = match (NonNull::new(item), unsafe { error.as_ref() }) {
        (_, Some(error)) => Err(describe(error)),
        (Some(item), None) => Ok(wrap(item)),
        (None, None) => Err(CaptureError::Backend("結果が空でした".into())),
    };
    let _ = tx.send(Delivered(result));
}

fn shareable_content() -> Result<Retained<SCShareableContent>, CaptureError> {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: 通知の間は生きているので、保持してから渡す
            deliver(&tx, content, error, |c| unsafe {
                Retained::retain(c.as_ptr()).expect("NULL ではない")
            });
        },
    );
    // SAFETY: handler は通知が終わるまで block のコピーとして保持される
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &handler,
        );
    }
    wait(&rx)
}

/// 窓の番号を手前から順に返す。
fn window_ids_front_to_back() -> Result<Vec<u32>, CaptureError> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let ids: CFRetained<CFArray> = CGWindowListCreate(options, 0)
        .ok_or_else(|| CaptureError::Backend("窓の順番を取得できませんでした".into()))?;
    let count = ids.count();
    // SAFETY: この配列は要素に CGWindowID をポインタの値として直接入れている
    Ok((0..count)
        .map(|at| unsafe { ids.value_at_index(at) } as usize as u32)
        .collect())
}

/// 最も手前の窓を取得する。窓の情報を返すために `SCWindow` を覚えておく。
#[derive(Default)]
pub struct MacCapturer {
    windows: HashMap<u64, Retained<SCWindow>>,
}

impl MacCapturer {
    /// 権限がなければ [`CaptureError::PermissionDenied`] を返す。
    pub fn new() -> Result<Self, CaptureError> {
        // CLI で動かすときに、ScreenCaptureKit より先に CoreGraphics を初期化する必要がある
        let _ = CGMainDisplayID();
        if !has_permission() {
            return Err(CaptureError::PermissionDenied);
        }
        Ok(Self::default())
    }
}

fn to_info(window: &SCWindow) -> Option<WindowInfo> {
    // SAFETY: どれも読み取りだけのプロパティ
    unsafe {
        let frame = window.frame();
        if window.windowLayer() != 0
            || !window.isOnScreen()
            || frame.size.width < MIN_WINDOW_POINTS
            || frame.size.height < MIN_WINDOW_POINTS
        {
            return None;
        }
        let app = window.owningApplication()?;
        Some(WindowInfo {
            id: window.windowID() as u64,
            pid: app.processID() as u32,
            app_id: app.bundleIdentifier().to_string(),
            app_name: app.applicationName().to_string(),
            title: window.title().map(|title| title.to_string()),
        })
    }
}

impl WindowCapturer for MacCapturer {
    fn list_windows(&mut self) -> Result<Vec<WindowInfo>, CaptureError> {
        if !has_permission() {
            return Err(CaptureError::PermissionDenied);
        }
        autoreleasepool(|_| {
            let content = shareable_content()?;
            let mut by_id = HashMap::new();
            // SAFETY: 読み取りだけのプロパティ
            for window in unsafe { content.windows() }.iter() {
                by_id.insert(unsafe { window.windowID() } as u64, window);
            }
            let mut listed = Vec::new();
            for id in window_ids_front_to_back()? {
                if let Some(info) = by_id.get(&(id as u64)).and_then(|w| to_info(w)) {
                    listed.push(info);
                }
            }
            self.windows = by_id;
            Ok(listed)
        })
    }

    fn capture(&mut self, window: &WindowInfo) -> Result<Frame, CaptureError> {
        let target = self
            .windows
            .get(&window.id)
            .ok_or_else(|| CaptureError::Backend("窓が閉じられました".into()))?;
        autoreleasepool(|_| capture_window(target))
    }
}

/// `desktopIndependentWindow` のフィルタは、その窓だけを写す。重なった窓は写らない。
fn capture_window(window: &SCWindow) -> Result<Frame, CaptureError> {
    let (tx, rx) = mpsc::channel();
    // SAFETY: 引数は生きている窓で、設定は開始前に済ませる
    let image = unsafe {
        let filter =
            SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), window);
        let scale = filter.pointPixelScale() as f64;
        let rect = filter.contentRect();
        let config = SCStreamConfiguration::new();
        config.setWidth((rect.size.width * scale).ceil() as usize);
        config.setHeight((rect.size.height * scale).ceil() as usize);
        config.setShowsCursor(false);
        config.setIgnoreShadowsSingleWindow(true);
        config.setShouldBeOpaque(true);
        let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
            deliver(&tx, image, error, |i| {
                // SAFETY: 通知の間は生きているので、保持してから渡す
                CFRetained::retain(i)
            });
        });
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            &filter,
            &config,
            Some(&handler),
        );
        wait(&rx)?
    };
    frame_from_image(&image)
}

/// 形式をそろえるため、BGRA のビットマップへ描き直す。
fn frame_from_image(image: &CGImage) -> Result<Frame, CaptureError> {
    let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    let stride = width * 4;
    let mut bgra = vec![0u8; stride * height];
    let color_space = CGColorSpace::new_device_rgb()
        .ok_or_else(|| CaptureError::Backend("色空間を作れませんでした".into()))?;
    // SAFETY: bgra は context より長く生き、大きさは幅と高さに合っている
    unsafe {
        let context = CGBitmapContextCreate(
            bgra.as_mut_ptr().cast(),
            width,
            height,
            8,
            stride,
            Some(&color_space),
            BGRA_BITMAP_INFO,
        )
        .ok_or_else(|| CaptureError::Backend("描画先を作れませんでした".into()))?;
        let rect = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        );
        CGContext::draw_image(Some(&context), rect, Some(image));
    }
    Ok(Frame {
        width: width as u32,
        height: height as u32,
        stride,
        bgra,
    })
}

fn image_from_frame(frame: &Frame) -> Result<CFRetained<CGImage>, CaptureError> {
    let fail = || CaptureError::Backend("画像を作れませんでした".into());
    let color_space = CGColorSpace::new_device_rgb().ok_or_else(fail)?;
    // SAFETY: 長さは bgra の長さそのもの
    let data = unsafe { CFData::new(None, frame.bgra.as_ptr(), frame.bgra.len() as isize) }
        .ok_or_else(fail)?;
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or_else(fail)?;
    // SAFETY: 幅、高さ、行のバイト数は bgra の並びと一致している
    unsafe {
        CGImage::new(
            frame.width as usize,
            frame.height as usize,
            8,
            32,
            frame.stride,
            Some(&color_space),
            CGBitmapInfo(BGRA_BITMAP_INFO),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(fail)
}

/// Vision の読み方の設定。比較の測定のために切り替えられるようにする。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisionOptions {
    pub accurate: bool,
    pub language_correction: bool,
}

impl Default for VisionOptions {
    fn default() -> Self {
        Self {
            accurate: true,
            language_correction: true,
        }
    }
}

/// Vision の OCR。日本語を先に指定する。
pub struct VisionRecognizer {
    options: VisionOptions,
}

const LANGUAGES: [&str; 2] = ["ja-JP", "en-US"];

fn new_request(options: VisionOptions) -> Result<Retained<VNRecognizeTextRequest>, CaptureError> {
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(if options.accurate {
        VNRequestTextRecognitionLevel::Accurate
    } else {
        VNRequestTextRecognitionLevel::Fast
    });
    request.setUsesLanguageCorrection(options.language_correction);
    let languages: Vec<_> = LANGUAGES.iter().map(|l| NSString::from_str(l)).collect();
    request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));
    Ok(request)
}

impl VisionRecognizer {
    /// 日本語を読めない環境なら [`CaptureError::Unsupported`] を返す。
    pub fn new() -> Result<Self, CaptureError> {
        Self::with_options(VisionOptions::default())
    }

    pub fn with_options(options: VisionOptions) -> Result<Self, CaptureError> {
        autoreleasepool(|_| {
            let request = new_request(options)?;
            // SAFETY: 読み取りだけの問い合わせ
            let supported = unsafe { request.supportedRecognitionLanguagesAndReturnError() }
                .map_err(|e| {
                    CaptureError::Unsupported(format!("言語を調べられません: {}", e.code()))
                })?;
            if !supported.iter().any(|l| l.to_string() == "ja-JP") {
                return Err(CaptureError::Unsupported(
                    "Vision が日本語に対応していません".into(),
                ));
            }
            Ok(Self { options })
        })
    }
}

impl TextRecognizer for VisionRecognizer {
    fn recognize(&mut self, frame: &Frame) -> Result<Vec<OcrLine>, CaptureError> {
        autoreleasepool(|_| {
            let image = image_from_frame(frame)?;
            let request = new_request(self.options)?;
            let options = NSDictionary::<objc2_vision::VNImageOption, AnyObject>::new();
            // SAFETY: image は処理の間生きている。同期の呼び出しで、結果もこの中で読む
            let handler = unsafe {
                VNImageRequestHandler::initWithCGImage_options(
                    VNImageRequestHandler::alloc(),
                    &image,
                    &options,
                )
            };
            let as_request: Retained<VNRequest> =
                Retained::into_super(Retained::into_super(request.clone()));
            handler
                .performRequests_error(&NSArray::from_retained_slice(&[as_request]))
                .map_err(|e| CaptureError::Backend(format!("Vision: {}", e.code())))?;

            let mut lines = Vec::new();
            for observation in request.results().iter().flat_map(|r| r.iter()) {
                let Some(candidate) = observation.topCandidates(1).firstObject() else {
                    continue;
                };
                // Vision の座標は左下が原点。ここで左上の原点へ直す
                let bounds = unsafe { observation.boundingBox() };
                lines.push(OcrLine {
                    text: candidate.string().to_string(),
                    confidence: candidate.confidence(),
                    x: bounds.origin.x as f32,
                    y: (1.0 - bounds.origin.y - bounds.size.height) as f32,
                    height: bounds.size.height as f32,
                });
            }
            sort_reading_order(&mut lines);
            Ok(lines)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 窓の番号は値として入っている() {
        // 配列の要素がポインタなら、32 ビットに収まらない
        let options = CGWindowListOption::OptionOnScreenOnly;
        let ids = CGWindowListCreate(options, 0).expect("一覧を取得できる");
        for at in 0..ids.count() {
            let id = unsafe { ids.value_at_index(at) } as usize;
            assert!(id <= u32::MAX as usize);
        }
    }

    #[test]
    fn 画像の変換で画素が変わらない() {
        let bgra: Vec<u8> = (0..16u8)
            .flat_map(|i| [i * 10, 200 - i * 5, i * 3, 255])
            .collect();
        let frame = Frame {
            width: 4,
            height: 4,
            stride: 16,
            bgra,
        };
        let image = image_from_frame(&frame).unwrap();
        assert_eq!(frame_from_image(&image).unwrap(), frame);
    }
}

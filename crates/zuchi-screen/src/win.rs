//! Windows の実装。Windows.Graphics.Capture で窓を取得し、Windows.Media.Ocr で読む。
//!
//! 手元に Windows の検証機がないので、コンパイルの確認までで、実機では動かしていない。

use std::time::{Duration, Instant};

use windows::Foundation::Rect;
use windows::Globalization::Language;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{CloseHandle, HMODULE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_FLAG, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_RESOURCE_MISC_FLAG, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};
use windows::core::{BOOL, HSTRING, Interface, PWSTR, Result as WinResult, factory, h};

use crate::error::CaptureError;
use crate::exclude::WindowInfo;
use crate::frame::Frame;
use crate::ocr::{OcrLine, join_words, sort_reading_order};
use crate::pipeline::{TextRecognizer, WindowCapturer};

/// 小さな窓は読まない。
const MIN_WINDOW_PIXELS: i32 = 160;
/// 最初の画像を待つ上限。
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

fn backend(context: &str, error: windows::core::Error) -> CaptureError {
    CaptureError::Backend(format!("{context}: {:#010x}", error.code().0))
}

/// パスから実行ファイル名を取り出す。除外の一覧はファイル名で比べる。
fn exe_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn executable_of(pid: u32) -> Option<String> {
    // SAFETY: 開いたハンドルはこの関数の中で閉じる
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        Some(exe_name(&path).to_string())
    }
}

fn window_title(hwnd: HWND) -> Option<String> {
    // SAFETY: バッファの長さを渡している
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return None;
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        Some(String::from_utf16_lossy(&buffer[..copied.max(0) as usize]))
    }
}

/// 通常の階層で、実際に見えている窓だけを選ぶ。
fn is_candidate(hwnd: HWND) -> bool {
    // SAFETY: hwnd は EnumWindows から渡された値で、失敗は見えない窓として扱う
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if style & (WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0) != 0 {
            return false;
        }
        let mut cloaked = 0u32;
        let cloak_result = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        );
        if cloak_result.is_err() || cloaked != 0 {
            return false;
        }
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).is_ok()
            && rect.right - rect.left >= MIN_WINDOW_PIXELS
            && rect.bottom - rect.top >= MIN_WINDOW_PIXELS
    }
}

unsafe extern "system" fn collect_window(hwnd: HWND, list: LPARAM) -> BOOL {
    // SAFETY: list_windows が渡した Vec で、列挙の間だけ使う
    let list = unsafe { &mut *(list.0 as *mut Vec<WindowInfo>) };
    if is_candidate(hwnd) {
        let mut pid = 0;
        // SAFETY: pid への書き込みだけ
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let exe = executable_of(pid).unwrap_or_default();
        list.push(WindowInfo {
            id: hwnd.0 as usize as u64,
            pid,
            app_name: exe.trim_end_matches(".exe").to_string(),
            app_id: exe,
            title: window_title(hwnd),
        });
    }
    true.into()
}

/// 最も手前の窓を、取得のたびに 1 枚だけ撮る。
pub struct GraphicsCapturer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
}

impl GraphicsCapturer {
    pub fn new() -> Result<Self, CaptureError> {
        let supported =
            GraphicsCaptureSession::IsSupported().map_err(|e| backend("IsSupported", e))?;
        if !supported {
            return Err(CaptureError::Unsupported(
                "Windows.Graphics.Capture が使えません".into(),
            ));
        }
        let mut device = None;
        let mut context = None;
        // SAFETY: 出力先はこの関数のローカル変数
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|e| backend("D3D11CreateDevice", e))?;
        let missing = || CaptureError::Backend("Direct3D の初期化に失敗しました".into());
        let device: ID3D11Device = device.ok_or_else(missing)?;
        let context = context.ok_or_else(missing)?;
        let dxgi: IDXGIDevice = device.cast().map_err(|e| backend("IDXGIDevice", e))?;
        // SAFETY: dxgi は有効なデバイス
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .map_err(|e| backend("CreateDirect3D11DeviceFromDXGIDevice", e))?;
        let winrt_device = inspectable
            .cast()
            .map_err(|e| backend("IDirect3DDevice", e))?;
        Ok(Self {
            device,
            context,
            winrt_device,
        })
    }

    fn copy_to_frame(&self, texture: &ID3D11Texture2D) -> WinResult<Frame> {
        let mut description = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: 出力先はローカル変数。Map した範囲は Unmap の前に読み終える
        unsafe {
            texture.GetDesc(&mut description);
            description.Usage = D3D11_USAGE_STAGING;
            description.BindFlags = D3D11_BIND_FLAG(0).0 as u32;
            description.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            description.MiscFlags = D3D11_RESOURCE_MISC_FLAG(0).0 as u32;
            let mut staging = None;
            self.device
                .CreateTexture2D(&description, None, Some(&mut staging))?;
            let staging = staging.ok_or_else(windows::core::Error::empty)?;
            self.context.CopyResource(&staging, texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let stride = mapped.RowPitch as usize;
            let height = description.Height as usize;
            let bgra =
                std::slice::from_raw_parts(mapped.pData as *const u8, stride * height).to_vec();
            self.context.Unmap(&staging, 0);
            Ok(Frame {
                width: description.Width,
                height: description.Height,
                stride,
                bgra,
            })
        }
    }

    fn capture_hwnd(&self, hwnd: HWND) -> WinResult<Option<Frame>> {
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        // SAFETY: hwnd は直前に列挙した窓。閉じられていれば失敗が返る
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd) }?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &self.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        session.SetIsCursorCaptureEnabled(false)?;
        // 枠の非表示は Windows 11 の API。失敗しても写る内容は変わらない
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture()?;

        let deadline = Instant::now() + FRAME_TIMEOUT;
        let mut result = None;
        while Instant::now() < deadline {
            if let Ok(captured) = pool.TryGetNextFrame() {
                let access: IDirect3DDxgiInterfaceAccess = captured.Surface()?.cast()?;
                // SAFETY: surface は captured が閉じられるまで有効
                let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
                result = Some(self.copy_to_frame(&texture)?);
                captured.Close()?;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        session.Close()?;
        pool.Close()?;
        Ok(result)
    }
}

impl WindowCapturer for GraphicsCapturer {
    fn list_windows(&mut self) -> Result<Vec<WindowInfo>, CaptureError> {
        let mut list: Vec<WindowInfo> = Vec::new();
        // SAFETY: コールバックは列挙の間だけ list を使う。EnumWindows は手前から順に渡す
        unsafe { EnumWindows(Some(collect_window), LPARAM(&mut list as *mut _ as isize)) }
            .map_err(|e| backend("EnumWindows", e))?;
        Ok(list)
    }

    fn capture(&mut self, window: &WindowInfo) -> Result<Frame, CaptureError> {
        let hwnd = HWND(window.id as usize as *mut _);
        self.capture_hwnd(hwnd)
            .map_err(|e| backend("capture", e))?
            .ok_or_else(|| CaptureError::Backend("画像が届きませんでした".into()))
    }
}

/// Windows.Media.Ocr。日本語の言語パックが入っていないと使えない。
pub struct WindowsRecognizer {
    engine: OcrEngine,
}

impl WindowsRecognizer {
    pub fn new() -> Result<Self, CaptureError> {
        let language: HSTRING = h!("ja").clone();
        let language = Language::CreateLanguage(&language).map_err(|e| backend("Language", e))?;
        let engine = OcrEngine::TryCreateFromLanguage(&language).map_err(|_| {
            CaptureError::Unsupported("日本語の OCR の言語パックが入っていません".into())
        })?;
        Ok(Self { engine })
    }

    fn read(&self, frame: &Frame) -> WinResult<Vec<OcrLine>> {
        let writer = DataWriter::new()?;
        let row = frame.width as usize * 4;
        for y in 0..frame.height as usize {
            writer.WriteBytes(&frame.bgra[y * frame.stride..][..row])?;
        }
        let buffer = writer.DetachBuffer()?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            frame.width as i32,
            frame.height as i32,
        )?;
        let result = self.engine.RecognizeAsync(&bitmap)?.join()?;
        let (width, height) = (frame.width as f32, frame.height as f32);
        let mut lines = Vec::new();
        for line in result.Lines()? {
            let mut words = Vec::new();
            let mut bounds: Option<Rect> = None;
            for word in line.Words()? {
                words.push(word.Text()?.to_string_lossy());
                let rect = word.BoundingRect()?;
                bounds.get_or_insert(rect);
            }
            let bounds = bounds.unwrap_or_default();
            lines.push(OcrLine {
                text: join_words(&words),
                // Windows.Media.Ocr は確信度を返さない
                confidence: 1.0,
                x: bounds.X / width,
                y: bounds.Y / height,
                height: bounds.Height / height,
            });
        }
        sort_reading_order(&mut lines);
        Ok(lines)
    }
}

impl TextRecognizer for WindowsRecognizer {
    fn recognize(&mut self, frame: &Frame) -> Result<Vec<OcrLine>, CaptureError> {
        self.read(frame).map_err(|e| backend("OCR", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 実行ファイル名を取り出す() {
        assert_eq!(
            exe_name(r"C:\Program Files\1Password\1Password.exe"),
            "1Password.exe"
        );
        assert_eq!(exe_name("app.exe"), "app.exe");
    }
}

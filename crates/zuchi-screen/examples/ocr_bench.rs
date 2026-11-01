//! 画像ファイルで OCR の精度と時間を測る。画面の権限は要らない。
//!
//! `<名前>.png` と、正解を 1 行ずつ書いた `<名前>.txt` を同じフォルダに置いて使う。
//! 手順は docs/検証-画面.md を参照。
//!
//! ```text
//! cargo run -p zuchi-screen --release --example ocr_bench -- <フォルダ> [--filter 語] [--repeat N]
//! cargo run -p zuchi-screen --release --example ocr_bench -- <フォルダ> --filter _2x --simulate 60 --change-every 8
//! ```
//!
//! macOS では `--fast` で速い読み方に、`--no-correction` で言語補正なしに切り替える。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use zuchi_screen::diff::normalize_line;
use zuchi_screen::pipeline::Pipeline;
use zuchi_screen::{
    CaptureError, Frame, OcrLine, PlatformRecognizer, ScreenConfig, ScreenStats, TextRecognizer,
    WindowCapturer, WindowInfo,
};

fn load_png(path: &Path) -> Frame {
    let file = fs::File::open(path).expect("画像を開けません");
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().expect("PNG を読めません");
    let mut pixels = vec![0; reader.output_buffer_size().expect("画像が大きすぎます")];
    let info = reader.next_frame(&mut pixels).expect("PNG を読めません");
    let channels = info.color_type.samples();
    assert!(channels >= 3, "RGB か RGBA の PNG を使ってください");
    let mut bgra = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    for pixel in pixels[..info.buffer_size()].chunks_exact(channels) {
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    Frame {
        width: info.width,
        height: info.height,
        stride: info.width as usize * 4,
        bgra,
    }
}

/// 正解の行に最も近い、OCR の結果の部分文字列との編集距離(Sellers のアルゴリズム)。
///
/// 行の順番の違いや、行の分割と結合を誤りに数えないために、全体の比較はしない。
fn best_substring_distance(needle: &[char], haystack: &[char]) -> usize {
    let mut previous = vec![0usize; haystack.len() + 1];
    for (i, x) in needle.iter().enumerate() {
        let mut current = vec![i + 1; haystack.len() + 1];
        for (j, y) in haystack.iter().enumerate() {
            current[j + 1] = (previous[j] + usize::from(x != y))
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        previous = current;
    }
    previous.into_iter().min().unwrap_or(needle.len())
}

#[derive(Default)]
struct Score {
    chars: usize,
    errors: usize,
    lines: usize,
    exact_lines: usize,
}

impl Score {
    fn add(&mut self, other: &Score) {
        self.chars += other.chars;
        self.errors += other.errors;
        self.lines += other.lines;
        self.exact_lines += other.exact_lines;
    }
    fn cer(&self) -> f64 {
        100.0 * self.errors as f64 / self.chars.max(1) as f64
    }
    fn exact(&self) -> f64 {
        100.0 * self.exact_lines as f64 / self.lines.max(1) as f64
    }
}

fn score(truth: &str, lines: &[OcrLine]) -> Score {
    let joined: Vec<char> = lines
        .iter()
        .flat_map(|line| normalize_line(&line.text).chars().collect::<Vec<_>>())
        .collect();
    let mut result = Score::default();
    for line in truth.lines() {
        let line: Vec<char> = normalize_line(line).chars().collect();
        let distance = best_substring_distance(&line, &joined);
        result.chars += line.len();
        result.errors += distance;
        result.lines += 1;
        result.exact_lines += usize::from(distance == 0);
    }
    result
}

/// 指定の間隔で画像を替える、窓が 1 つだけの偽の取得。実際の画面は取得しない。
struct ReplayCapturer {
    frames: Vec<Frame>,
    change_every: usize,
    ticks: usize,
}

impl WindowCapturer for ReplayCapturer {
    fn list_windows(&mut self) -> Result<Vec<WindowInfo>, CaptureError> {
        Ok(vec![WindowInfo {
            id: 1,
            pid: u32::MAX,
            app_id: "bench".into(),
            app_name: "bench".into(),
            title: Some("bench".into()),
        }])
    }

    fn capture(&mut self, _window: &WindowInfo) -> Result<Frame, CaptureError> {
        let at = self.ticks / self.change_every % self.frames.len();
        self.ticks += 1;
        Ok(self.frames[at].clone())
    }
}

/// 処理全体で使った CPU 時間と、最大のメモリ。
#[cfg(unix)]
fn process_usage() -> (Duration, u64) {
    // SAFETY: 出力先はゼロで初期化した構造体
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    let time = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    (
        time(usage.ru_utime) + time(usage.ru_stime),
        usage.ru_maxrss as u64,
    )
}

#[cfg(not(unix))]
fn process_usage() -> (Duration, u64) {
    (Duration::ZERO, 0)
}

fn recognizer(args: &[String]) -> PlatformRecognizer {
    #[cfg(target_os = "macos")]
    let result = PlatformRecognizer::with_options(zuchi_screen::macos::VisionOptions {
        accurate: !args.iter().any(|a| a == "--fast"),
        language_correction: !args.iter().any(|a| a == "--no-correction"),
    });
    #[cfg(not(target_os = "macos"))]
    let result = {
        let _ = args;
        PlatformRecognizer::new()
    };
    result.expect("OCR を準備できません")
}

fn simulate(mut paths: Vec<PathBuf>, seconds: u64, change_every: usize, args: &[String]) {
    paths.truncate(8);
    let frames = paths.iter().map(|p| load_png(p)).collect();
    let config = ScreenConfig::default();
    let stats = Arc::new(ScreenStats::default());
    let capturer = ReplayCapturer {
        frames,
        change_every,
        ticks: 0,
    };
    let mut pipeline = Pipeline::new(capturer, recognizer(args), &config, Arc::clone(&stats));
    let (cpu_start, _) = process_usage();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        let tick = Instant::now();
        pipeline.tick().expect("処理に失敗しました");
        std::thread::sleep(config.interval.saturating_sub(tick.elapsed()));
    }
    let wall = start.elapsed();
    let (cpu_end, rss) = process_usage();
    println!(
        "周期 {} 回、OCR {} 回、流した数 {}",
        stats.ticks.load(Ordering::Relaxed),
        stats.ocr_runs.load(Ordering::Relaxed),
        stats.emitted.load(Ordering::Relaxed),
    );
    println!(
        "CPU {:.1}%(1 コアを 100%)、最大メモリ {} MB",
        100.0 * (cpu_end - cpu_start).as_secs_f64() / wall.as_secs_f64(),
        rss / 1024 / 1024,
    );
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).map(String::as_str)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let directory = args.first().expect("画像のフォルダを指定してください");
    let filter = option(&args, "--filter").unwrap_or("");
    let repeat: usize = option(&args, "--repeat").map_or(3, |v| v.parse().expect("--repeat"));
    let mut paths: Vec<PathBuf> = fs::read_dir(directory)
        .expect("フォルダを開けません")
        .map(|entry| entry.expect("フォルダを読めません").path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .filter(|p| p.to_string_lossy().contains(filter))
        .collect();
    paths.sort();

    if let Some(seconds) = option(&args, "--simulate") {
        let change_every = option(&args, "--change-every").map_or(8, |v| v.parse().expect("数"));
        simulate(
            paths,
            seconds.parse().expect("--simulate"),
            change_every,
            &args,
        );
        return;
    }

    let mut engine = recognizer(&args);
    println!("| 画像 | 大きさ | 時間(中央値) | CER | 完全一致の行 |");
    println!("|---|---|---:|---:|---:|");
    let mut total = Score::default();
    let mut medians = Vec::new();
    for path in paths {
        let frame = load_png(&path);
        let truth = fs::read_to_string(path.with_extension("txt")).expect("正解がありません");
        let mut times = Vec::new();
        let mut lines = Vec::new();
        for _ in 0..repeat.max(1) {
            let start = Instant::now();
            lines = engine.recognize(&frame).expect("OCR に失敗しました");
            times.push(start.elapsed());
        }
        times.sort();
        let median = times[times.len() / 2];
        medians.push(median);
        let result = score(&truth, &lines);
        println!(
            "| {} | {}x{} | {:.0} ms | {:.1}% | {}/{} |",
            path.file_stem().unwrap_or_default().to_string_lossy(),
            frame.width,
            frame.height,
            median.as_secs_f64() * 1000.0,
            result.cer(),
            result.exact_lines,
            result.lines,
        );
        total.add(&result);
    }
    medians.sort();
    println!(
        "\n全体: CER {:.2}%、完全一致の行 {:.1}%、時間の中央値 {:.0} ms",
        total.cer(),
        total.exact(),
        medians
            .get(medians.len() / 2)
            .map_or(0.0, |m| m.as_secs_f64() * 1000.0),
    );
}

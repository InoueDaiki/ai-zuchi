//! 窓関数つき sinc によるリサンプラ。音声はチャンク単位で届くので、状態を持って続きから処理する。

use std::f64::consts::PI;

/// 出力のサンプルレート。
pub const TARGET_RATE: u32 = 16_000;

/// sinc の片側のゼロ交差の数。大きいほど急峻になり、計算が増える
const ZERO_CROSSINGS: usize = 24;
/// 係数表の位相の数
const PHASES: usize = 256;
/// ダウンサンプル時に、折り返しを防ぐためカットオフをナイキストより下げる割合
const CUTOFF_MARGIN: f64 = 0.92;

/// モノラルの信号のサンプルレートを変える。
pub struct Resampler {
    /// 出力 1 サンプルあたりの入力の進み
    step: f64,
    /// 片側のタップ数
    half: usize,
    /// 位相ごとの係数。`table[phase * (2 * half) + i]`
    table: Vec<f32>,
    /// まだ使い切っていない入力。先頭に履歴を含む
    buffer: Vec<f32>,
    /// `buffer` の中の次の出力位置
    position: f64,
    passthrough: bool,
}

impl Resampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        let passthrough = input_rate == output_rate;
        let step = f64::from(input_rate) / f64::from(output_rate);
        let cutoff = if input_rate > output_rate {
            CUTOFF_MARGIN / step
        } else {
            1.0
        };
        let half = (ZERO_CROSSINGS as f64 / cutoff).ceil() as usize;

        let mut table = Vec::new();
        if !passthrough {
            table.reserve(PHASES * 2 * half);
            for phase in 0..PHASES {
                let fraction = phase as f64 / PHASES as f64;
                let mut taps: Vec<f64> = (0..2 * half)
                    .map(|i| {
                        let x = i as f64 - (half as f64 - 1.0) - fraction;
                        sinc(cutoff * x) * blackman(x / half as f64)
                    })
                    .collect();
                // 直流の利得を 1 に揃える
                let sum: f64 = taps.iter().sum();
                taps.iter_mut().for_each(|t| *t /= sum);
                table.extend(taps.iter().map(|&t| t as f32));
            }
        }

        Self {
            step,
            half,
            table,
            // 最初の出力が入力の先頭に対応するように、ゼロで埋めておく
            buffer: vec![0.0; half - 1],
            position: 0.0,
            passthrough,
        }
    }

    /// `input` を処理し、できた分を `output` の末尾に足す。
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if self.passthrough {
            output.extend_from_slice(input);
            return;
        }
        self.buffer.extend_from_slice(input);

        let width = 2 * self.half;
        loop {
            let start = self.position.floor() as usize;
            if start + width > self.buffer.len() {
                break;
            }
            let fraction = self.position - start as f64;
            let phase = ((fraction * PHASES as f64) as usize).min(PHASES - 1);
            let taps = &self.table[phase * width..(phase + 1) * width];
            let value = taps
                .iter()
                .zip(&self.buffer[start..start + width])
                .map(|(t, s)| t * s)
                .sum();
            output.push(value);
            self.position += self.step;
        }

        let consumed = (self.position.floor() as usize).min(self.buffer.len());
        self.buffer.drain(..consumed);
        self.position -= consumed as f64;
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

/// `x` は -1..=1。
fn blackman(x: f64) -> f64 {
    if x.abs() >= 1.0 {
        return 0.0;
    }
    let t = PI * x;
    0.42 + 0.5 * t.cos() + 0.08 * (2.0 * t).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f64, rate: u32, seconds: f64) -> Vec<f32> {
        let count = (f64::from(rate) * seconds) as usize;
        (0..count)
            .map(|i| (2.0 * PI * frequency * i as f64 / f64::from(rate)).sin() as f32 * 0.5)
            .collect()
    }

    fn rms(samples: &[f32]) -> f64 {
        // 端の過渡応答を除く
        let middle = &samples[samples.len() / 4..samples.len() * 3 / 4];
        (middle.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / middle.len() as f64).sqrt()
    }

    fn run(rate: u32, input: &[f32]) -> Vec<f32> {
        let mut resampler = Resampler::new(rate, TARGET_RATE);
        let mut output = Vec::new();
        resampler.process(input, &mut output);
        output
    }

    #[test]
    fn 同じレートならそのまま通す() {
        let input = sine(440.0, TARGET_RATE, 0.1);
        assert_eq!(run(TARGET_RATE, &input), input);
    }

    #[test]
    fn 通過帯域の音量を保つ() {
        for rate in [8_000, 44_100, 48_000, 96_000] {
            let input = sine(1_000.0, rate, 1.0);
            let output = run(rate, &input);
            let ratio = rms(&output) / rms(&input);
            assert!((ratio - 1.0).abs() < 0.01, "{rate} Hz: 比 {ratio}");
        }
    }

    #[test]
    fn 折り返す帯域を落とす() {
        for rate in [44_100, 48_000, 96_000] {
            let input = sine(12_000.0, rate, 1.0);
            let output = run(rate, &input);
            let db = 20.0 * (rms(&output) / rms(&input)).log10();
            assert!(db < -60.0, "{rate} Hz: {db} dB");
        }
    }

    #[test]
    fn 出力の長さはレートの比になる() {
        for rate in [44_100, 48_000] {
            let output = run(rate, &sine(440.0, rate, 1.0));
            let expected = TARGET_RATE as usize;
            assert!(output.len().abs_diff(expected) < 64, "{}", output.len());
        }
    }

    #[test]
    fn 分割して渡しても結果は変わらない() {
        let input = sine(1_000.0, 48_000, 0.5);
        let whole = run(48_000, &input);

        let mut resampler = Resampler::new(48_000, TARGET_RATE);
        let mut split = Vec::new();
        for chunk in input.chunks(137) {
            resampler.process(chunk, &mut split);
        }
        assert_eq!(whole, split);
    }

    #[test]
    fn 出力は入力と時間がずれない() {
        // 48 kHz の入力の 3 サンプルごとに 16 kHz の 1 サンプルが対応する
        let input = sine(200.0, 48_000, 0.5);
        let output = run(48_000, &input);
        for (i, &value) in output.iter().enumerate().skip(200).take(1_000) {
            assert!((value - input[i * 3]).abs() < 0.01, "{i}");
        }
    }
}

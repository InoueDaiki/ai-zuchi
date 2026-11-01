//! 縮小画像の差分で画面の変化を検知する。OCR を呼ぶ回数を減らすための段階。

use crate::frame::Frame;

/// 変化の検知の設定。
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeConfig {
    /// 縮小画像の幅と高さ
    pub thumbnail_size: (u32, u32),
    /// 1 マスの輝度の差がこれ以上なら、そのマスは変わったとみなす
    pub cell_delta: u8,
    /// 変わったマスの割合がこれ以上なら、画面が変わったとみなす
    pub changed_ratio: f32,
    /// 変わり続けていても、この回数ごとに一度は読む。動画やスクロールで読めなくなるのを防ぐ
    pub max_settling_ticks: u32,
}

impl Default for ChangeConfig {
    fn default() -> Self {
        Self {
            thumbnail_size: (160, 90),
            cell_delta: 6,
            // 160 x 90 のうち約 30 マス。カーソルの点滅や時計の 1 桁では反応しない
            changed_ratio: 0.002,
            max_settling_ticks: 5,
        }
    }
}

/// 輝度だけの縮小画像。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub luma: Vec<u8>,
}

impl Thumbnail {
    /// 面積平均で縮小する。間引くと細い文字の変化を取りこぼす。
    pub fn from_frame(frame: &Frame, width: u32, height: u32) -> Self {
        let (out_w, out_h) = (width.max(1) as usize, height.max(1) as usize);
        let (in_w, in_h) = (frame.width as usize, frame.height as usize);
        let mut sums = vec![0u32; out_w * out_h];
        let mut counts = vec![0u32; out_w * out_h];
        let columns: Vec<usize> = (0..in_w).map(|x| x * out_w / in_w).collect();
        for y in 0..in_h {
            let row_out = (y * out_h / in_h) * out_w;
            let row = &frame.bgra[y * frame.stride..][..in_w * 4];
            for (pixel, column) in row.as_chunks::<4>().0.iter().zip(&columns) {
                let [b, g, r] = [pixel[0] as u32, pixel[1] as u32, pixel[2] as u32];
                // ITU-R BT.601 の輝度を整数で近似する
                sums[row_out + column] += (77 * r + 150 * g + 29 * b) >> 8;
                counts[row_out + column] += 1;
            }
        }
        let luma = sums
            .iter()
            .zip(&counts)
            .map(|(sum, count)| (sum / (*count).max(1)) as u8)
            .collect();
        Self {
            width: out_w as u32,
            height: out_h as u32,
            luma,
        }
    }
}

/// 変わったマスの割合。大きさが違えば全部変わったものとする。
pub fn changed_ratio(a: &Thumbnail, b: &Thumbnail, cell_delta: u8) -> f32 {
    if (a.width, a.height) != (b.width, b.height) || a.luma.is_empty() {
        return 1.0;
    }
    let changed = a
        .luma
        .iter()
        .zip(&b.luma)
        .filter(|(x, y)| x.abs_diff(**y) >= cell_delta)
        .count();
    changed as f32 / a.luma.len() as f32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeDecision {
    /// 前に読んだときから変わっていない
    Unchanged,
    /// 変わったが、まだ動いている。次の周期まで待つ
    Settling,
    /// OCR で読む
    Read,
}

/// 前に読んだ画面と比べ、落ち着いたところで読むように決める。
#[derive(Debug)]
pub struct ChangeDetector {
    config: ChangeConfig,
    /// 前に読んだときの縮小画像
    reference: Option<Thumbnail>,
    /// 1 周期前の縮小画像
    previous: Option<Thumbnail>,
    settling_ticks: u32,
}

impl ChangeDetector {
    pub fn new(config: ChangeConfig) -> Self {
        Self {
            config,
            reference: None,
            previous: None,
            settling_ticks: 0,
        }
    }

    pub fn thumbnail_of(&self, frame: &Frame) -> Thumbnail {
        let (width, height) = self.config.thumbnail_size;
        Thumbnail::from_frame(frame, width, height)
    }

    /// 窓が替わったときや、読むのをやめたときに、状態を捨てる。
    pub fn reset(&mut self) {
        self.reference = None;
        self.previous = None;
        self.settling_ticks = 0;
    }

    fn differs(&self, a: &Thumbnail, b: &Thumbnail) -> bool {
        changed_ratio(a, b, self.config.cell_delta) >= self.config.changed_ratio
    }

    pub fn observe(&mut self, thumbnail: Thumbnail) -> ChangeDecision {
        let previous = self.previous.replace(thumbnail.clone());
        let Some(reference) = &self.reference else {
            self.reference = Some(thumbnail);
            return ChangeDecision::Read;
        };
        if !self.differs(reference, &thumbnail) {
            self.settling_ticks = 0;
            return ChangeDecision::Unchanged;
        }
        let settled = previous.is_some_and(|p| !self.differs(&p, &thumbnail));
        self.settling_ticks += 1;
        if settled || self.settling_ticks >= self.config.max_settling_ticks {
            self.reference = Some(thumbnail);
            self.settling_ticks = 0;
            return ChangeDecision::Read;
        }
        ChangeDecision::Settling
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 白地の画像を作り、指定の矩形を黒く塗る。
    fn frame(width: u32, height: u32, black: &[(u32, u32, u32, u32)]) -> Frame {
        let stride = width as usize * 4;
        let mut bgra = vec![255u8; stride * height as usize];
        for &(x0, y0, w, h) in black {
            for y in y0..y0 + h {
                for x in x0..x0 + w {
                    let at = y as usize * stride + x as usize * 4;
                    bgra[at..at + 3].fill(0);
                }
            }
        }
        Frame {
            width,
            height,
            stride,
            bgra,
        }
    }

    fn detector() -> ChangeDetector {
        ChangeDetector::new(ChangeConfig::default())
    }

    #[test]
    fn 縮小は面積平均になる() {
        let thumbnail = Thumbnail::from_frame(&frame(4, 2, &[(0, 0, 2, 2)]), 2, 1);
        assert_eq!(thumbnail.luma[0], 0);
        assert!(thumbnail.luma[1] >= 250);
    }

    #[test]
    fn 行の余りがある画像も縮小できる() {
        let mut image = frame(4, 2, &[]);
        let stride = 20;
        image.bgra = vec![255; stride * 2];
        image.stride = stride;
        assert_eq!(Thumbnail::from_frame(&image, 2, 2).luma.len(), 4);
    }

    #[test]
    fn 大きさが違えば全部変わった扱いになる() {
        let a = Thumbnail::from_frame(&frame(8, 8, &[]), 4, 4);
        let b = Thumbnail::from_frame(&frame(8, 8, &[]), 2, 2);
        assert_eq!(changed_ratio(&a, &b, 6), 1.0);
    }

    #[test]
    fn 初回は読み同じ画面は読まない() {
        let mut d = detector();
        let image = frame(1600, 900, &[]);
        assert_eq!(d.observe(d.thumbnail_of(&image)), ChangeDecision::Read);
        assert_eq!(d.observe(d.thumbnail_of(&image)), ChangeDecision::Unchanged);
    }

    #[test]
    fn カーソルの点滅には反応しない() {
        let mut d = detector();
        let before = frame(1600, 900, &[]);
        let after = frame(1600, 900, &[(800, 400, 3, 18)]);
        d.observe(d.thumbnail_of(&before));
        assert_eq!(d.observe(d.thumbnail_of(&after)), ChangeDecision::Unchanged);
        assert_eq!(
            d.observe(d.thumbnail_of(&before)),
            ChangeDecision::Unchanged
        );
    }

    #[test]
    fn 新しい一行は落ち着いてから読む() {
        let mut d = detector();
        let before = frame(1600, 900, &[]);
        // 細い横線が 6 本で、1 行の文字に近い濃さの変化
        let bars: Vec<_> = (0..6).map(|i| (100, 500 + i * 3, 700, 1)).collect();
        let after = frame(1600, 900, &bars);
        d.observe(d.thumbnail_of(&before));
        assert_eq!(d.observe(d.thumbnail_of(&after)), ChangeDecision::Settling);
        assert_eq!(d.observe(d.thumbnail_of(&after)), ChangeDecision::Read);
        assert_eq!(d.observe(d.thumbnail_of(&after)), ChangeDecision::Unchanged);
    }

    #[test]
    fn 変わり続ける画面は間隔をあけて読む() {
        let mut d = detector();
        d.observe(d.thumbnail_of(&frame(320, 180, &[])));
        let mut decisions = Vec::new();
        for i in 0..10 {
            let image = frame(320, 180, &[(i * 30, 0, 30, 180)]);
            decisions.push(d.observe(d.thumbnail_of(&image)));
        }
        let reads = decisions
            .iter()
            .filter(|x| **x == ChangeDecision::Read)
            .count();
        assert_eq!(reads, 2);
        assert_eq!(decisions[4], ChangeDecision::Read);
    }

    #[test]
    fn 元の画面へ戻れば待ちを取り消す() {
        let mut d = detector();
        let before = frame(320, 180, &[]);
        d.observe(d.thumbnail_of(&before));
        let popup = frame(320, 180, &[(0, 0, 160, 90)]);
        assert_eq!(d.observe(d.thumbnail_of(&popup)), ChangeDecision::Settling);
        assert_eq!(
            d.observe(d.thumbnail_of(&before)),
            ChangeDecision::Unchanged
        );
    }

    #[test]
    fn リセットすると次は読む() {
        let mut d = detector();
        let image = frame(320, 180, &[]);
        d.observe(d.thumbnail_of(&image));
        d.reset();
        assert_eq!(d.observe(d.thumbnail_of(&image)), ChangeDecision::Read);
    }
}

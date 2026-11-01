//! 直近の音声をメモリに持つリングバッファ。
//!
//! VAD が区間を返した時点で、その前後の実音声を取り出すために使う。

use std::collections::VecDeque;

pub(crate) struct AudioRing {
    samples: VecDeque<f32>,
    capacity: usize,
    /// これまでに受け取ったサンプル数。VAD の位置と同じ数え方
    end: u64,
}

impl AudioRing {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            end: 0,
        }
    }

    pub(crate) fn end(&self) -> u64 {
        self.end
    }

    pub(crate) fn push(&mut self, chunk: &[f32]) {
        for &sample in chunk {
            if self.samples.len() == self.capacity {
                self.samples.pop_front();
            }
            self.samples.push_back(sample);
        }
        self.end += chunk.len() as u64;
    }

    /// 通算の位置 `[start, end)` を取り出す。残っていない部分は切り詰める。
    pub(crate) fn range(&self, start: u64, end: u64) -> Vec<f32> {
        let first = self.end - self.samples.len() as u64;
        let start = start.max(first);
        let end = end.min(self.end);
        if start >= end {
            return Vec::new();
        }
        self.samples
            .iter()
            .skip((start - first) as usize)
            .take((end - start) as usize)
            .copied()
            .collect()
    }

    pub(crate) fn clear(&mut self) {
        self.samples.clear();
        self.end = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 通算の位置で_取り出せる() {
        let mut ring = AudioRing::new(4);
        ring.push(&[0.0, 1.0, 2.0]);
        ring.push(&[3.0, 4.0, 5.0]);
        assert_eq!(ring.end(), 6);
        assert_eq!(ring.range(3, 5), vec![3.0, 4.0]);
    }

    #[test]
    fn 範囲の外は_切り詰める() {
        let mut ring = AudioRing::new(4);
        ring.push(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        // 0 と 1 は押し出されている。6 以降はまだ来ていない
        assert_eq!(ring.range(0, 100), vec![2.0, 3.0, 4.0, 5.0]);
        assert!(ring.range(0, 2).is_empty());
        assert!(ring.range(6, 8).is_empty());
    }

    #[test]
    fn 消すと_位置も_最初に戻る() {
        let mut ring = AudioRing::new(4);
        ring.push(&[1.0, 2.0]);
        ring.clear();
        assert_eq!(ring.end(), 0);
        ring.push(&[3.0]);
        assert_eq!(ring.range(0, 1), vec![3.0]);
    }
}

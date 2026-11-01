//! メモリ上の画像。ディスクには書かない。

/// BGRA 8 ビットの画像。OCR の後すぐに捨てる。
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// 1 行のバイト数。`width * 4` より大きいことがある
    pub stride: usize,
    pub bgra: Vec<u8>,
}

/// 画面の内容がログに出ないよう、大きさだけを出す。
impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

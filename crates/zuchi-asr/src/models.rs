//! モデルのファイルの場所。

use std::path::{Path, PathBuf};

use zuchi_core::ports::Result;

const ASR_DIR: &str = "sherpa-onnx-zipformer-ja-reazonspeech-2024-08-01";

/// `mise run models:fetch` が置くファイルの一覧。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelPaths {
    pub vad: PathBuf,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl ModelPaths {
    /// `models/` に相当するディレクトリから組み立てる。ファイルの有無は見ない。
    pub fn in_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        let asr = dir.join(ASR_DIR);
        Self {
            vad: dir.join("silero_vad.onnx"),
            encoder: asr.join("encoder-epoch-99-avg-1.int8.onnx"),
            decoder: asr.join("decoder-epoch-99-avg-1.int8.onnx"),
            joiner: asr.join("joiner-epoch-99-avg-1.int8.onnx"),
            tokens: asr.join("tokens.txt"),
        }
    }

    /// すべてのファイルがあるか。
    pub fn exist(&self) -> bool {
        [
            &self.vad,
            &self.encoder,
            &self.decoder,
            &self.joiner,
            &self.tokens,
        ]
        .iter()
        .all(|path| path.is_file())
    }
}

/// sherpa-onnx はファイルがないと異常終了することがあるので、渡す前に確かめる。
pub(crate) fn checked_path(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Err(format!(
            "モデルがない: {}(`mise run models:fetch` で取得する)",
            path.display()
        )
        .into());
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("UTF-8 でないパスは扱えない: {}", path.display()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ない_ファイルは_取得の方法を_知らせる() {
        let paths = ModelPaths::in_dir("存在しないディレクトリ");
        assert!(!paths.exist());
        let message = checked_path(&paths.vad).unwrap_err().to_string();
        assert!(message.contains("models:fetch"));
    }
}

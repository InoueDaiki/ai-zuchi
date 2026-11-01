#!/usr/bin/env bash
# VAD と ASR のモデルを models/ へ取得する。取得済みなら何もしない。
# 取得元は sherpa-onnx の GitHub Releases。SHA-256 を確かめてから置く。
set -euo pipefail

BASE="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models"
MODELS_DIR="models"
ASR_NAME="sherpa-onnx-zipformer-ja-reazonspeech-2024-08-01"
ASR_SHA256="e0981d0d5d7b446d41010831b59091ebf57d2aa7b79980f67ca37af460b5842d"
VAD_NAME="silero_vad.onnx"
VAD_SHA256="9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6"

verify() {
  local file="$1" expected="$2" actual
  if command -v sha256sum >/dev/null; then
    actual="$(sha256sum "$file" | cut -d' ' -f1)"
  else
    actual="$(shasum -a 256 "$file" | cut -d' ' -f1)"
  fi
  if [ "$actual" != "$expected" ]; then
    echo "SHA-256 が一致しない: $file" >&2
    rm -f "$file"
    exit 1
  fi
}

mkdir -p "$MODELS_DIR"

if [ ! -f "$MODELS_DIR/$VAD_NAME" ]; then
  curl -fL --retry 3 -o "$MODELS_DIR/$VAD_NAME.part" "$BASE/$VAD_NAME"
  verify "$MODELS_DIR/$VAD_NAME.part" "$VAD_SHA256"
  mv "$MODELS_DIR/$VAD_NAME.part" "$MODELS_DIR/$VAD_NAME"
fi

if [ ! -d "$MODELS_DIR/$ASR_NAME" ]; then
  archive="$MODELS_DIR/$ASR_NAME.tar.bz2"
  curl -fL --retry 3 -C - -o "$archive" "$BASE/$ASR_NAME.tar.bz2"
  verify "$archive" "$ASR_SHA256"
  # 途中で失敗しても、不完全なディレクトリを取得済みと誤認しないようにする
  tmp="$MODELS_DIR/.extract"
  rm -rf "$tmp"
  mkdir "$tmp"
  tar -xjf "$archive" -C "$tmp"
  mv "$tmp/$ASR_NAME" "$MODELS_DIR/$ASR_NAME"
  rm -rf "$tmp" "$archive"
fi

echo "モデルは $MODELS_DIR/ にある"

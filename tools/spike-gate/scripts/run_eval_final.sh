#!/bin/bash
# 評価用を流す。プロンプトを固定したあとに 1 度だけ実行する。
# Qwen の think あり全部と、qwen3:4b の free は tune で p50 が 60 秒を超えたので除いた。
set -u
cd "$(dirname "$0")/.."
bash scripts/run_all.sh eval gemma4:e4b gemma4:e4b-it-q4_K_M gemma4:e2b
uv run python scripts/run_eval.py --model qwen3:4b --schema --no-think --split eval
uv run python scripts/run_eval.py --model qwen3.5:4b --schema --no-think --split eval
uv run python scripts/run_eval.py --model qwen3.5:4b --no-schema --no-think --split eval
uv run python scripts/run_calc.py --split tune --models gemma4:e4b gemma4:e4b-it-q4_K_M gemma4:e2b qwen3:4b qwen3.5:4b
uv run python scripts/run_calc.py --split eval --models gemma4:e4b gemma4:e4b-it-q4_K_M gemma4:e2b qwen3:4b qwen3.5:4b
uv run python scripts/score.py --split tune
uv run python scripts/score.py --split eval

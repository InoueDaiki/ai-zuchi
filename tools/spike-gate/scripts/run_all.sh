#!/bin/bash
# 全構成を順に流す。LLM の呼び出しは同時に 1 つだけ。
# 使い方: run_all.sh <tune|eval> モデル...
set -u
cd "$(dirname "$0")/.."
split=$1; shift
for model in "$@"; do
  for s in --schema --no-schema; do
    for t in --no-think --think; do
      uv run python scripts/run_eval.py --model "$model" "$s" "$t" --split "$split"
    done
  done
done

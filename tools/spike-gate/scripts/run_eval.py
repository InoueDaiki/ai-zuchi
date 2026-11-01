"""判定を 1 構成ぶん流し、results/raw/ に JSONL で保存する。

例: run_eval.py --model gemma4:e4b --schema --no-think --split tune
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path

import yaml

import ollama_client as oc
import prompts

ROOT = Path(__file__).resolve().parent.parent


def load_scenes(split: str) -> list[dict]:
    return yaml.safe_load((ROOT / "data" / f"{split}.yaml").read_text())["scenes"]


def config_name(model: str, schema: bool, think: bool) -> str:
    return re.sub(r"[:/]", "-", model) + ("-schema" if schema else "-free") + ("-think" if think else "")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--schema", action=argparse.BooleanOptionalAction, default=True)
    ap.add_argument("--think", action=argparse.BooleanOptionalAction, default=False)
    ap.add_argument("--split", choices=["tune", "eval"], default="tune")
    ap.add_argument("--limit", type=int)
    ap.add_argument("--tag", default="", help="出力名に足す")
    ap.add_argument("--num-predict", type=int, default=2048)
    args = ap.parse_args()

    scenes = load_scenes(args.split)[: args.limit]
    name = config_name(args.model, args.schema, args.think) + (f"-{args.tag}" if args.tag else "")
    out_dir = ROOT / "results" / "raw" / args.split
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / f"{name}.jsonl"

    oc.unload(args.model)
    time.sleep(1)
    meta = {
        "type": "meta",
        "model": args.model,
        "schema": args.schema,
        "think": args.think,
        "split": args.split,
        "prompt_hash": prompts.prompt_hash(),
        "options": oc.OPTIONS,
        "num_predict": args.num_predict,
        "loadavg_start": os.getloadavg(),
    }
    # 初回ロードを別に測る。この 1 件は集計に入れない
    warm = oc.chat(
        args.model,
        prompts.gate_messages(scenes[0], use_schema=args.schema),
        schema=prompts.GATE_SCHEMA if args.schema else None,
        think=args.think,
        num_predict=args.num_predict,
    )
    meta["warmup"] = {k: warm.get(k) for k in ("wall_ms", "load_ms", "error")}

    rows = []
    mem_peak: dict = {}
    for i, scene in enumerate(scenes):
        res = oc.chat(
            args.model,
            prompts.gate_messages(scene, use_schema=args.schema),
            schema=prompts.GATE_SCHEMA if args.schema else None,
            think=args.think,
            num_predict=args.num_predict,
        )
        parsed = oc.parse_json(res.get("content", "")) if "error" not in res else None
        rows.append({"type": "row", "id": scene["id"], "parsed": parsed, **res})
        if i % 10 == 0:
            snap = oc.memory_snapshot(args.model)
            for k, v in snap.items():
                mem_peak[k] = max(mem_peak.get(k, 0), v) if isinstance(v, (int, float)) else v
            print(f"{name} {i + 1}/{len(scenes)} {res.get('wall_ms', 0):.0f}ms", file=sys.stderr, flush=True)
    meta["memory"] = mem_peak
    meta["loadavg_end"] = os.getloadavg()
    with out.open("w") as f:
        f.write(json.dumps(meta, ensure_ascii=False) + "\n")
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    oc.unload(args.model)


if __name__ == "__main__":
    main()

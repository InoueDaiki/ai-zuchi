"""計算スキルの検証。正解が calc の場面で LLM に数値を抽出させ、コードで換算して正解と比べる。"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import yaml

import calc
import ollama_client as oc
import prompts
from score import pct

ROOT = Path(__file__).resolve().parent.parent


def expected(gold: dict) -> dict:
    if gold["op"] == "rate":
        return {"per_day": gold["total"] / gold["days"]}
    if gold["op"] == "divide":
        return {"result": gold["total"] / gold["by"]}
    return {"result": gold["a"] * gold["b"]}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", default="tune")
    ap.add_argument("--models", nargs="+", required=True)
    args = ap.parse_args()
    scenes = [s for s in yaml.safe_load((ROOT / "data" / f"{args.split}.yaml").read_text())["scenes"] if s["skill"] == "calc"]
    summary = []
    out_dir = ROOT / "results" / "raw" / "calc"
    out_dir.mkdir(parents=True, exist_ok=True)
    for model in args.models:
        for use_schema in (True, False):
            rows, wall = [], []
            # 初回ロードの 1 件は時間の集計に入れない
            oc.chat(model, prompts.extract_messages(scenes[0]), schema=prompts.EXTRACT_SCHEMA if use_schema else None, think=False, num_predict=512)
            for s in scenes:
                r = oc.chat(model, prompts.extract_messages(s), schema=prompts.EXTRACT_SCHEMA if use_schema else None, think=False, num_predict=512)
                parsed = oc.parse_json(r.get("content", ""))
                row = {"id": s["id"], "parsed": parsed, "wall_ms": r.get("wall_ms"), "gold": s["calc"]}
                wall.append(r.get("wall_ms", 0))
                try:
                    got = calc.convert(parsed or {})
                    row["text"] = got["text"]
                    row["values"] = got["values"]
                    want = expected(s["calc"])
                    row["ok"] = all(abs(got["values"].get(k, 0) - v) <= abs(v) * 0.01 for k, v in want.items())
                except (KeyError, ValueError, TypeError) as e:
                    row["ok"] = False
                    row["convert_error"] = repr(e)
                rows.append(row)
            name = model.replace(":", "-") + ("-schema" if use_schema else "-free")
            (out_dir / f"{args.split}-{name}.jsonl").write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in rows) + "\n")
            ok = sum(r["ok"] for r in rows)
            summary.append({"name": name, "ok": ok, "n": len(rows), "p50_ms": pct(wall, 0.5), "p95_ms": pct(wall, 0.95), "failed": [r["id"] for r in rows if not r["ok"]]})
            print(summary[-1], flush=True)
        oc.unload(model)
    (ROOT / "results" / f"calc-{args.split}.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()

"""同じ eval を 3 周流し、メモリの推移、結果の再現性、ロード後の遅延を測る。プロンプトの調整には使わない。"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

import ollama_client as oc
import prompts
from run_eval import load_scenes
from score import pct, predict

ROOT = Path(__file__).resolve().parent.parent


def main() -> None:
    scenes = load_scenes("eval")
    report = {}
    for model in sys.argv[1:]:
        oc.unload(model)
        time.sleep(2)
        preds: list[dict] = []
        trace, wall = [], []
        n = 0
        for lap in range(3):
            lap_pred = {}
            for s in scenes:
                r = oc.chat(model, prompts.gate_messages(s, use_schema=True), schema=prompts.GATE_SCHEMA, think=False, num_predict=512)
                lap_pred[s["id"]] = predict({"parsed": oc.parse_json(r.get("content", ""))})
                n += 1
                if n > 1:
                    wall.append(r.get("wall_ms", 0))
                if n % 24 == 0 or n == 1:
                    trace.append({"calls": n, **oc.memory_snapshot(model)})
            preds.append(lap_pred)
        same = [sum(preds[0][k] == p[k] for k in preds[0]) for p in preds[1:]]
        report[model] = {
            "calls": n,
            "identical_to_lap1": same,
            "wall_p50_ms": pct(wall, 0.5),
            "wall_p95_ms": pct(wall, 0.95),
            "memory_trace": trace,
        }
        print(model, same, report[model]["wall_p50_ms"], report[model]["wall_p95_ms"], trace[0], trace[-1], flush=True)
        oc.unload(model)
    (ROOT / "results" / "soak.json").write_text(json.dumps(report, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()

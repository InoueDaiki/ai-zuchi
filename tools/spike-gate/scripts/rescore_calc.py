"""保存済みの抽出結果を、今の calc.py で換算し直す。LLM は呼ばない。

値の正誤に加えて、補足文の単位が正解の単位と合うかも数える。
"""

from __future__ import annotations

import json
from pathlib import Path

import calc
from run_calc import expected

ROOT = Path(__file__).resolve().parent.parent


def main() -> None:
    out = []
    for path in sorted((ROOT / "results" / "raw" / "calc").glob("eval-*.jsonl")):
        rows = [json.loads(l) for l in path.read_text().splitlines()]
        ok = unit_ok = 0
        failed = []
        for r in rows:
            gold = r["gold"]
            try:
                got = calc.convert(r["parsed"] or {})
                want = expected(gold)
                good = all(abs(got["values"].get(k, 0) - v) <= abs(v) * 0.01 for k, v in want.items())
            except (KeyError, ValueError, TypeError):
                got, good = None, False
            if not good:
                failed.append(r["id"])
                continue
            ok += 1
            p = r["parsed"]
            unit = gold.get("unit") if gold["op"] != "multiply" else gold.get("a_unit")
            used = p.get("unit") if gold["op"] != "multiply" else p.get("a_unit")
            unit_ok += used == unit
        out.append({"name": path.stem, "n": len(rows), "value_ok": ok, "value_and_unit_ok": unit_ok, "failed": failed})
        print(out[-1])
    (ROOT / "results" / "calc-eval-rescored.json").write_text(json.dumps(out, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()

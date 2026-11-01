"""results/raw/<split>/*.jsonl を採点し、results/summary-<split>.{json,md} に書く。

「口を出す」の適合率と再現率は、スキルを問わない値(発話の有無)と、スキルまで合った値(strict)の両方を出す。
strict の適合率が、設計メモの「補足の適合率」に近い。
"""

from __future__ import annotations

import argparse
import json
import math
from collections import Counter
from pathlib import Path

import yaml

import rules

ROOT = Path(__file__).resolve().parent.parent
THRESHOLDS = [0.0, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95]


def wilson(k: int, n: int, z: float = 1.96) -> tuple[float, float] | None:
    if n == 0:
        return None
    p = k / n
    d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    h = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return (c - h, c + h)


def pct(xs: list[float], q: float) -> float | None:
    if not xs:
        return None
    s = sorted(xs)
    return s[min(len(s) - 1, math.ceil(q * len(s)) - 1)]


def predict(row: dict, threshold: float = 0.0) -> str:
    """応答をスキルに直す。壊れた応答と、しきい値未満は黙る扱いにする。"""
    p = row.get("parsed")
    if not isinstance(p, dict) or p.get("skill") not in ("calc", "search", "memory", "suggest"):
        return "none"
    conf = p.get("confidence")
    if threshold > 0 and (not isinstance(conf, (int, float)) or conf < threshold):
        return "none"
    return p["skill"]


def metrics(scenes: list[dict], pred: dict[str, str]) -> dict:
    speak_gold = [s for s in scenes if s["skill"] != "none"]
    silent_gold = [s for s in scenes if s["skill"] == "none"]
    tp = sum(pred[s["id"]] != "none" for s in speak_gold)
    strict = sum(pred[s["id"]] == s["skill"] for s in speak_gold)
    fp = sum(pred[s["id"]] != "none" for s in silent_gold)
    said = tp + fp
    m = {
        "n": len(scenes),
        "n_speak": len(speak_gold),
        "n_silent": len(silent_gold),
        "said": said,
        "precision": tp / said if said else None,
        "precision_strict": strict / said if said else None,
        "recall": tp / len(speak_gold) if speak_gold else None,
        "recall_strict": strict / len(speak_gold) if speak_gold else None,
        "skill_acc_when_spoken": strict / tp if tp else None,
        "false_speech": fp,
        "false_speech_rate": fp / len(silent_gold) if silent_gold else None,
        "wrong_skill": tp - strict,
        "missed": len(speak_gold) - tp,
    }
    p = strict / said if said else 0
    r = strict / len(speak_gold) if speak_gold else 0
    m["f05_strict"] = (1.25 * p * r / (0.25 * p + r)) if p + r else 0
    m["precision_strict_ci"] = wilson(strict, said)
    m["recall_strict_ci"] = wilson(strict, len(speak_gold))
    m["false_speech_ci"] = wilson(fp, len(silent_gold))
    return m


def score_file(path: Path, scenes: list[dict]) -> dict | None:
    lines = [json.loads(l) for l in path.read_text().splitlines()]
    meta = lines[0]
    rows = {r["id"]: r for r in lines[1:]}
    if len(rows) < len(scenes):
        scenes = [s for s in scenes if s["id"] in rows]
    pred = {s["id"]: predict(rows[s["id"]]) for s in scenes}
    res = {"name": path.stem, "meta": meta, **metrics(scenes, pred)}
    res["json_ok"] = sum(isinstance(rows[s["id"]].get("parsed"), dict) for s in scenes) / len(scenes)
    res["errors"] = sum("error" in rows[s["id"]] for s in scenes)
    res["empty"] = sum(not rows[s["id"]].get("content", "").strip() for s in scenes)
    res["length_stop"] = sum(rows[s["id"]].get("done_reason") == "length" for s in scenes)
    wall = [rows[s["id"]]["wall_ms"] for s in scenes if "error" not in rows[s["id"]]]
    res["latency_ms"] = {"p50": pct(wall, 0.5), "p95": pct(wall, 0.95), "max": max(wall) if wall else None}
    gen = [rows[s["id"]] for s in scenes if rows[s["id"]].get("gen_ms")]
    res["gen_tokens_mean"] = sum(r["gen_tokens"] for r in gen) / len(gen) if gen else None
    res["gen_tok_s"] = sum(r["gen_tokens"] for r in gen) / sum(r["gen_ms"] for r in gen) * 1000 if gen else None
    res["by_threshold"] = {
        str(t): {k: v for k, v in metrics(scenes, {s["id"]: predict(rows[s["id"]], t) for s in scenes}).items() if not k.endswith("_ci")}
        for t in THRESHOLDS
    }
    fp_reason = Counter(s.get("reason") for s in scenes if s["skill"] == "none" and pred[s["id"]] != "none")
    res["false_speech_by_reason"] = dict(fp_reason)
    total_reason = Counter(s.get("reason") for s in scenes if s["skill"] == "none")
    res["silent_by_reason"] = dict(total_reason)
    res["per_skill"] = {
        k: {
            "n": sum(s["skill"] == k for s in scenes),
            "hit": sum(s["skill"] == k and pred[s["id"]] == k for s in scenes),
            "predicted": sum(pred[s["id"]] == k for s in scenes),
        }
        for k in ("calc", "search", "memory", "suggest")
    }
    noisy = [s for s in scenes if "asr-noise" in (s.get("tags") or [])]
    if noisy:
        res["asr_noise"] = {k: v for k, v in metrics(noisy, pred).items() if not k.endswith("_ci")}
    res["usecases"] = {
        t: {s["id"]: pred[s["id"]] for s in scenes if t in (s.get("tags") or [])}
        for t in ("usecase-1", "usecase-2", "usecase-3", "usecase-4")
    }
    res["errors_detail"] = [
        {"id": s["id"], "gold": s["skill"], "pred": pred[s["id"]], "reason": s.get("reason")}
        for s in scenes
        if pred[s["id"]] != s["skill"]
    ]
    return res


def rule_analysis(scenes: list[dict], results: list[dict]) -> dict:
    """段階 2 の効果。LLM の予測は保存済みの結果を使い、ルールで落ちた場面は黙る扱いにする。"""
    variants = {
        "最後の発話のみ": lambda s: bool(rules.cues(s)),
        "直近2発話": lambda s: bool(rules.cues_recent(s, 2)),
    }
    out: dict = {"variants": {}}
    for name, fn in variants.items():
        passed = {s["id"] for s in scenes if fn(s)}
        speak = [s for s in scenes if s["skill"] != "none"]
        silent = [s for s in scenes if s["skill"] == "none"]
        out["variants"][name] = {
            "calls": len(passed),
            "calls_of": len(scenes),
            "dropped_good": [s["id"] for s in speak if s["id"] not in passed],
            "n_speak": len(speak),
            "silent_passed": sum(s["id"] in passed for s in silent),
            "n_silent": len(silent),
        }
    utterances = [str(t["text"]) for s in scenes for t in s["turns"]]
    out["utterance_pass_rate"] = sum(rules.passes_utterance(u) for u in utterances) / len(utterances)
    out["n_utterances"] = len(utterances)
    out["combined"] = {}
    for r in results:
        rows = {
            l["id"]: l
            for l in map(json.loads, (ROOT / "results" / "raw" / r["meta"]["split"] / f"{r['name']}.jsonl").read_text().splitlines())
            if l.get("type") == "row"
        }
        if len(rows) < len(scenes):
            continue
        for name, fn in variants.items():
            pred = {s["id"]: predict(rows[s["id"]]) if fn(s) else "none" for s in scenes}
            m = metrics(scenes, pred)
            out["combined"].setdefault(r["name"], {})[name] = {k: m[k] for k in ("said", "precision_strict", "recall_strict", "false_speech", "missed")}
    return out


def fmt(v, d=2):
    return "-" if v is None else f"{v:.{d}f}"


def markdown(results: list[dict]) -> str:
    head = "| 構成 | 発話数 | 適合率(有無) | 適合率(スキルまで) | 再現率(スキルまで) | 誤発話(黙るべき場面) | スキル誤り | JSON | p50 秒 | p95 秒 | tok/s | 生成 tok |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n"
    out = [head]
    for r in sorted(results, key=lambda r: -(r["f05_strict"] or 0)):
        l = r["latency_ms"]
        out.append(
            f"| {r['name']} | {r['said']} | {fmt(r['precision'])} | {fmt(r['precision_strict'])} | {fmt(r['recall_strict'])} | "
            f"{r['false_speech']}/{r['n_silent']} | {r['wrong_skill']} | {fmt(r['json_ok'])} | {fmt(l['p50'] and l['p50'] / 1000)} | {fmt(l['p95'] and l['p95'] / 1000)} | "
            f"{fmt(r['gen_tok_s'], 1)} | {fmt(r['gen_tokens_mean'], 0)} |\n"
        )
    return "".join(out)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", default="tune")
    ap.add_argument("--errors", help="名前に一致する構成の誤りを表示する(tune 専用)")
    args = ap.parse_args()
    scenes = yaml.safe_load((ROOT / "data" / f"{args.split}.yaml").read_text())["scenes"]
    results = [score_file(p, scenes) for p in sorted((ROOT / "results" / "raw" / args.split).glob("*.jsonl"))]
    results = [r for r in results if r]
    summary = {"results": results, "rules": rule_analysis(scenes, results)}
    (ROOT / "results" / f"summary-{args.split}.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1))
    md = markdown(results)
    (ROOT / "results" / f"summary-{args.split}.md").write_text(md)
    print(md)
    if args.errors:
        if args.split == "eval":
            raise SystemExit("eval の誤りは調整が終わるまで見ない")
        for r in results:
            if args.errors in r["name"]:
                print(r["name"], r["errors_detail"])


if __name__ == "__main__":
    main()

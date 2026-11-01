"""Ollama の HTTP API(localhost だけ)を呼ぶ。文字起こしを外へ送らないよう、接続先は固定する。"""

from __future__ import annotations

import json
import re
import subprocess
import time
import urllib.error
import urllib.request

BASE = "http://localhost:11434"
OPTIONS = {"temperature": 0, "top_k": 1, "seed": 42, "num_ctx": 4096}


def _post(path: str, body: dict, timeout: float) -> dict:
    req = urllib.request.Request(
        BASE + path,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as res:
        return json.load(res)


def chat(
    model: str,
    messages: list[dict],
    *,
    schema: dict | None,
    think: bool,
    num_predict: int,
    timeout: float = 600,
) -> dict:
    """1 件の判定を流し、応答と時間を返す。"""
    body = {
        "model": model,
        "messages": messages,
        "stream": False,
        # think は省略すると既定値がモデルで異なるので、必ず明示する
        "think": think,
        "keep_alive": "30m",
        "options": {**OPTIONS, "num_predict": num_predict},
    }
    if schema is not None:
        body["format"] = schema
    started = time.perf_counter()
    try:
        res = _post("/api/chat", body, timeout)
    except urllib.error.HTTPError as e:
        return {"error": f"HTTP {e.code}: {e.read()[:300]!r}", "wall_ms": (time.perf_counter() - started) * 1000}
    except (urllib.error.URLError, TimeoutError) as e:
        return {"error": repr(e), "wall_ms": (time.perf_counter() - started) * 1000}
    wall_ms = (time.perf_counter() - started) * 1000
    msg = res.get("message", {})
    return {
        "content": msg.get("content", ""),
        "thinking": msg.get("thinking", ""),
        "done_reason": res.get("done_reason"),
        "wall_ms": wall_ms,
        "total_ms": res.get("total_duration", 0) / 1e6,
        "load_ms": res.get("load_duration", 0) / 1e6,
        "prompt_tokens": res.get("prompt_eval_count", 0),
        "prompt_ms": res.get("prompt_eval_duration", 0) / 1e6,
        "gen_tokens": res.get("eval_count", 0),
        "gen_ms": res.get("eval_duration", 0) / 1e6,
    }


def parse_json(text: str) -> dict | None:
    """応答から JSON のオブジェクトを取り出す。コードブロックと前後の文は許す。"""
    # Qwen3 は think:false でも、本文に思考を書くことがある
    text = text.rsplit("</think>", 1)[-1]
    text = re.sub(r"```(?:json)?", "", text).strip()
    try:
        v = json.loads(text)
        return v if isinstance(v, dict) else None
    except json.JSONDecodeError:
        pass
    m = re.search(r"\{.*\}", text, re.S)
    if m:
        try:
            v = json.loads(m.group())
            return v if isinstance(v, dict) else None
        except json.JSONDecodeError:
            return None
    return None


def unload(model: str) -> None:
    try:
        _post("/api/generate", {"model": model, "keep_alive": 0, "prompt": ""}, 60)
    except (urllib.error.URLError, TimeoutError):
        pass


def memory_snapshot(model: str) -> dict:
    """読み込み中のメモリ。/api/ps の値と、ollama の runner の RSS と footprint を取る。"""
    with urllib.request.urlopen(BASE + "/api/ps", timeout=10) as res:
        ps = json.load(res)
    out: dict = {}
    for m in ps.get("models", []):
        if m["name"] == model:
            out["ps_size_gb"] = m.get("size", 0) / 1e9
            out["ps_vram_gb"] = m.get("size_vram", 0) / 1e9
            out["context_length"] = m.get("context_length")
    # runner は `ollama runner` か `llama-server` の名前で動く。/api/ps の値は llama-server で小さく出る
    lines = subprocess.run(["ps", "-axo", "pid=,rss=,command="], capture_output=True, text=True).stdout
    rss = [(int(p), int(r)) for p, r, c in (l.split(None, 2) for l in lines.splitlines() if l.strip()) if "llama-server" in c or ("ollama" in c and "runner" in c)]
    if rss:
        pid, kb = max(rss, key=lambda x: x[1])
        out["runner_rss_gb"] = kb * 1024 / 1e9
        try:
            fp = subprocess.run(["footprint", str(pid)], capture_output=True, text=True, timeout=30).stdout
            m = re.search(r"Footprint:\s*([\d.]+)\s*(KB|MB|GB)", fp) or re.search(r"phys_footprint:\s*([\d.]+)\s*(KB|MB|GB)", fp)
            if m:
                out["runner_footprint_gb"] = float(m.group(1)) * {"KB": 1e-6, "MB": 1e-3, "GB": 1}[m.group(2)]
        except (subprocess.TimeoutExpired, FileNotFoundError):
            pass
    return out

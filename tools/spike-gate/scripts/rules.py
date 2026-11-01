"""段階 2 の軽いルールの絞り込み。調整は data/tune.yaml だけで行う。

手がかりは緩く取る。判定の対象は最後の発話と画面の文字で、ここで落とすと LLM には届かない。
"""

from __future__ import annotations

import re
import unicodedata

NUM = r"[0-9０-９〇一二三四五六七八九十百千万億兆]"

CUES: dict[str, str] = {
    # 大きな数と単位。「3時」「5階」のように小さな数だけの発話は通さない
    "number": rf"{NUM}+(\.[0-9]+)?\s*[万億兆]|[0-9]{{3,}}|[0-9],[0-9]{{3}}|[0-9]+[kKmM]\b|千(件|人|円|台|個)",
    # 疑問、思い出す
    "question": r"(ですか|ますか|ましたか|でしたっけ|だっけ|たっけ|ですかね|ますかね|かな|かなあ|かなぁ|かね|だろう|でしょう|どうなん|なんなん|なんで|なぜ|どこ|どう|いつ|だれ|誰|何|なに|いくら|どのくらい|どれくらい|[?？])",
    # 依頼と期限
    "request": r"(ください|下さい|お願い|おねがい|までに|期限|締め切り|締切|リマインド|送ります|やっておきます|確認します|対応します|提出)",
    # 記憶
    "memory": r"(この前|このあいだ|こないだ|前回|前に|以前|先週|先月|決めた|決まった|言ってた|話した|覚えて)",
}

COMPILED = {k: re.compile(v) for k, v in CUES.items()}


def cues(scene: dict, *, use_screen: bool = True) -> list[str]:
    """場面の手がかりを返す。空なら LLM を呼ばない。"""
    text = unicodedata.normalize("NFKC", str(scene["turns"][-1]["text"]))
    hits = [k for k, r in COMPILED.items() if r.search(text)]
    if use_screen and scene.get("screen"):
        screen = unicodedata.normalize("NFKC", str(scene["screen"]))
        if COMPILED["request"].search(screen):
            hits.append("screen-request")
    return hits


def cues_recent(scene: dict, window: int = 2) -> list[str]:
    """直近の window 発話まで見る版。"""
    hits: set[str] = set()
    for t in scene["turns"][-window:]:
        hits |= set(cues({"turns": [t]}, use_screen=False))
    hits |= {c for c in cues({**scene, "turns": scene["turns"][-1:]}) if c == "screen-request"}
    return sorted(hits)


def passes_utterance(text: str) -> bool:
    """発話 1 つだけを見る。全発話の通過率の推定に使う。"""
    return bool(cues({"turns": [{"text": text}]}, use_screen=False))

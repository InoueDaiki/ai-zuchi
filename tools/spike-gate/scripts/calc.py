"""計算スキルの最小実装。LLM は数値と単位の抽出だけを行い、換算はここで行う。"""

from __future__ import annotations

import re
import unicodedata

# 期間の日数。3 カ月は設計メモのとおり 91 日とする
DAYS = {"秒": 1 / 86400, "分": 1 / 1440, "時間": 1 / 24, "日": 1, "週間": 7, "週": 7}
MONTH_DAYS = {1: 30, 2: 61, 3: 91, 6: 182, 12: 365}

KANJI_DIGITS = {"〇": 0, "零": 0, "一": 1, "二": 2, "三": 3, "四": 4, "五": 5, "六": 6, "七": 7, "八": 8, "九": 9}
SMALL_UNITS = {"十": 10, "百": 100, "千": 1000}
LARGE_UNITS = {"万": 10**4, "億": 10**8, "兆": 10**12}


def parse_number(text: str | int | float) -> float:
    """「10億」「2億4000万」「五千万」「1.5万」「1,280」を数値にする。読めなければ ValueError。"""
    if isinstance(text, bool):
        raise ValueError(text)
    if isinstance(text, (int, float)):
        return float(text)
    s = unicodedata.normalize("NFKC", str(text)).replace(",", "").replace(" ", "")
    s = re.sub(r"(くらい|ほど|約|およそ|弱|強|円|件|人)$", "", s)
    s = re.sub(r"^(約|およそ)", "", s)
    if not s:
        raise ValueError(text)
    total = 0.0
    section = 0.0  # 万、億の区切りの中の値
    current: float | None = None
    i = 0
    while i < len(s):
        m = re.match(r"\d+(\.\d+)?", s[i:])
        ch = s[i]
        if m:
            current = float(m.group())
            i += m.end()
        elif ch in KANJI_DIGITS:
            current = float(KANJI_DIGITS[ch]) if current is None else current * 10 + KANJI_DIGITS[ch]
            i += 1
        elif ch in SMALL_UNITS:
            section += (1 if current is None else current) * SMALL_UNITS[ch]
            current = None
            i += 1
        elif ch in LARGE_UNITS:
            section += current or 0
            total += (section or 1) * LARGE_UNITS[ch]
            section, current = 0.0, None
            i += 1
        else:
            raise ValueError(text)
    return total + section + (current or 0)


def period_days(text: str) -> float:
    """「3カ月」「1週間」「半年」「1日」「1時間」を日数にする。読めなければ ValueError。"""
    s = unicodedata.normalize("NFKC", str(text)).replace(" ", "")
    if s in ("半年",):
        return 182
    # 「月に」「年に」のように数がなければ 1 とみなす
    if s in ("月", "年", "週", "日", "毎月", "毎年", "毎週", "毎日"):
        s = "1" + s.removeprefix("毎")
    m = re.fullmatch(r"(.+?)(カ月|か月|ヶ月|ケ月|ヵ月|ヵげつ|月間|月|年間|年|週間|週|日間|日|時間|分間|分|秒間|秒)", s)
    if not m:
        raise ValueError(text)
    n = parse_number(m.group(1))
    unit = m.group(2)
    if unit in ("カ月", "か月", "ヶ月", "ケ月", "ヵ月", "月間", "月"):
        return MONTH_DAYS.get(int(n), round(n * 30.4375)) if n == int(n) else n * 30.4375
    if unit in ("年間", "年"):
        return n * 365
    unit = unit.removesuffix("間") if unit in ("日間", "分間", "秒間") else unit
    return n * DAYS[unit]


def compact(x: float, digits: int = 3) -> str:
    """有効数字 3 桁で、万、億を使って読みやすく書く。"""
    if x == 0:
        return "0"
    from decimal import Decimal, ROUND_HALF_UP

    d = Decimal(repr(x))
    exp = d.adjusted() - digits + 1
    q = d.quantize(Decimal(1).scaleb(exp), rounding=ROUND_HALF_UP) if exp > 0 else d
    if exp <= 0:
        q = d.quantize(Decimal(1).scaleb(min(exp, 0)), rounding=ROUND_HALF_UP)
    value = float(q)
    for base, name in ((10**12, "兆"), (10**8, "億"), (10**4, "万")):
        if value >= base:
            scaled = value / base
            text = f"{scaled:,.4f}".rstrip("0").rstrip(".")
            return f"{text}{name}"
    text = f"{value:,.4f}".rstrip("0").rstrip(".")
    return text


def convert(extraction: dict) -> dict:
    """抽出の結果を換算する。`{"values": {...}, "text": 補足文}` を返す。読めなければ ValueError。"""
    op = extraction.get("op")
    if op == "rate":
        total = parse_number(extraction["total"])
        days = period_days(extraction["period"])
        unit = extraction.get("unit") or ""
        per_day = total / days
        per_sec = per_day / 86400
        note = f"{extraction['period']}={days:g}日で計算"
        return {
            "values": {"per_day": per_day, "per_sec": per_sec, "days": days},
            "text": f"秒間約{compact(per_sec)}{unit}、1日約{compact(per_day)}{unit}({note})",
        }
    if op == "divide":
        total = parse_number(extraction["total"])
        by = parse_number(extraction["by"])
        if by == 0:
            raise ValueError("0 で割れない")
        v = total / by
        by_unit = extraction.get("by_unit") or ""
        return {
            "values": {"result": v},
            "text": f"1{by_unit}あたり約{compact(v)}{extraction.get('unit') or ''}",
        }
    if op == "multiply":
        a = parse_number(extraction["a"])
        b = parse_number(extraction["b"])
        v = a * b
        return {"values": {"result": v}, "text": f"約{compact(v)}{extraction.get('a_unit') or ''}"}
    raise ValueError(f"未対応の op: {op}")

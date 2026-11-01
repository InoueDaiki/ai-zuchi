import pytest

from calc import compact, convert, parse_number, period_days


@pytest.mark.parametrize(
    ("text", "want"),
    [
        ("10億", 1_000_000_000),
        ("2億4000万", 240_000_000),
        ("五千万", 50_000_000),
        ("1.5万", 15_000),
        ("1,280", 1280),
        ("3600万", 36_000_000),
        ("百二十", 120),
        ("約500件", 500),
        (4500, 4500),
    ],
)
def test_parse_number(text, want):
    assert parse_number(text) == want


@pytest.mark.parametrize("text", ["", "たくさん", "数百"])
def test_parse_number_rejects(text):
    with pytest.raises(ValueError):
        parse_number(text)


@pytest.mark.parametrize(
    ("text", "want"),
    [("3カ月", 91), ("3ヶ月", 91), ("三か月", 91), ("1カ月", 30), ("1年", 365), ("1週間", 7), ("半年", 182), ("1日", 1), ("月", 30), ("毎月", 30), ("年", 365)],
)
def test_period_days(text, want):
    assert period_days(text) == want


def test_usecase_1():
    """使用例 1: 3 カ月で 10 億件 → 秒間約 127 件、1 日約 1,100 万件(91 日)"""
    got = convert({"op": "rate", "total": "10億", "unit": "件", "period": "3カ月"})
    assert round(got["values"]["per_sec"]) == 127
    assert round(got["values"]["per_day"], -5) == 11_000_000
    assert got["values"]["days"] == 91
    assert got["text"] == "秒間約127件、1日約1,100万件(3カ月=91日で計算)"


def test_compact():
    assert compact(127.2) == "127"
    assert compact(10_989_011) == "1,100万"
    assert compact(1_600_000) == "160万"
    assert compact(0.42) == "0.42"


def test_divide_and_multiply():
    assert convert({"op": "divide", "total": "2億4000万", "unit": "円", "by": "15", "by_unit": "人"})["values"]["result"] == 16_000_000
    assert convert({"op": "multiply", "a": "1280", "a_unit": "円", "b": "4500", "b_unit": "人"})["values"]["result"] == 5_760_000

# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow==13.0.0"]
# ///
"""日本語の画面を想定した合成画像と正解の文字列を作る。

文面はすべて架空のもの。画像はコミットせず、target/screen-bench に作る。

    uv run crates/zuchi-screen/bench/gen_screens.py target/screen-bench
"""

import glob
import io
import sys
import unicodedata
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

WIDTH, HEIGHT = 1440, 900


def font_path(name: str) -> str:
    """ヒラギノのファイル名は濁点が分解されていることがあるので、正規化して探す。"""
    for path in glob.glob("/System/Library/Fonts/*"):
        if unicodedata.normalize("NFC", Path(path).name) == name:
            return path
    raise FileNotFoundError(name)


GOTHIC = font_path("ヒラギノ角ゴシック W3.ttc")
GOTHIC_BOLD = font_path("ヒラギノ角ゴシック W6.ttc")
MINCHO = font_path("ヒラギノ明朝 ProN.ttc")
MONO = font_path("Menlo.ttc")


class Canvas:
    def __init__(self, scale: int, background: str):
        self.scale = scale
        self.image = Image.new("RGB", (WIDTH * scale, HEIGHT * scale), background)
        self.draw = ImageDraw.Draw(self.image)
        self.truth: list[str] = []

    def rect(self, x, y, w, h, color):
        s = self.scale
        self.draw.rectangle((x * s, y * s, (x + w) * s, (y + h) * s), fill=color)

    def text(self, x, y, text, size=14, color="#1d1c1d", path=GOTHIC):
        font = ImageFont.truetype(path, size * self.scale)
        self.draw.text((x * self.scale, y * self.scale), text, font=font, fill=color)
        self.truth.append(text)


CHAT = [
    ("佐藤", "10:02", "おはようございます。来週の定例の資料、共有フォルダに置きました。"),
    ("田中", "10:05", "ありがとうございます!3カ月で10億件という想定は、前回の見積もりと同じですか?"),
    ("佐藤", "10:07", "はい、同じです。サーバーの費用は月額48万円で計算しています。"),
    ("鈴木", "10:12", "こちらご確認ください。11月14日(金)までにお返事をいただけると助かります。"),
    ("田中", "10:15", "承知しました。APIの仕様書はv2.3を見ればよいでしょうか?"),
    ("鈴木", "10:21", "最新はv2.4です。認証まわりの変更点だけ先に読んでおいてください。"),
    ("高橋", "10:34", "BONXの差別化ってなんなんですかね。競合の比較表も見てみたいです。"),
    ("佐藤", "10:40", "前に決めたあの件、どうなったか議事録を探してみます。"),
]
CHANNELS = ["全体連絡", "開発チーム", "デザイン相談", "営業共有", "雑談", "障害対応"]


def chat(c: Canvas, background, sidebar, foreground, muted):
    c.rect(0, 0, 240, HEIGHT, sidebar)
    c.text(20, 20, "架空株式会社", 18, "#ffffff", GOTHIC_BOLD)
    for i, name in enumerate(CHANNELS):
        c.text(24, 70 + i * 32, name, 15, "#d8d8d8")
    c.text(270, 22, "開発チーム", 18, foreground, GOTHIC_BOLD)
    c.rect(240, 62, WIDTH - 240, 1, muted)
    for i, (name, time, body) in enumerate(CHAT):
        y = 86 + i * 92
        c.rect(270, y, 40, 40, muted)
        c.text(326, y, name, 15, foreground, GOTHIC_BOLD)
        c.text(390, y + 2, time, 12, muted)
        c.text(326, y + 26, body, 15, foreground)


def chat_light(c):
    chat(c, "#ffffff", "#3f0e40", "#1d1c1d", "#868686")


def chat_dark(c):
    chat(c, "#1a1d21", "#19171d", "#d1d2d3", "#9a9b9e")


def mail(c):
    c.text(40, 30, "件名: 【ご確認のお願い】第3四半期の請求書について", 20, path=GOTHIC_BOLD)
    c.text(40, 74, "差出人: 山田 花子 <hanako@example.com>", 14, "#555555")
    c.text(40, 98, "宛先: 経理部 各位", 14, "#555555")
    c.text(40, 122, "日時: 2026年10月28日 14:32", 14, "#555555")
    c.rect(40, 152, WIDTH - 80, 1, "#dddddd")
    body = [
        "経理部の皆さま",
        "いつもお世話になっております。総務の山田です。",
        "第3四半期の請求書を添付いたしましたので、内容をご確認ください。",
        "合計金額は1,284,500円(税込)で、支払期限は2026年11月30日です。",
        "なお、先月分との差額は印刷費の増加によるものです。",
        "ご不明な点がございましたら、内線3021までご連絡をお願いいたします。",
        "引き続きどうぞよろしくお願いいたします。",
    ]
    for i, line in enumerate(body):
        c.text(40, 178 + i * 40, line, 16)


def table(c):
    columns = [(40, 0), (360, 1), (620, 2), (880, 3), (1120, 4)]
    rows = [
        ("項目", "4月", "5月", "6月", "合計"),
        ("売上高", "1,250万円", "1,380万円", "1,410万円", "4,040万円"),
        ("広告宣伝費", "210万円", "185万円", "240万円", "635万円"),
        ("人件費", "620万円", "620万円", "650万円", "1,890万円"),
        ("クラウド利用料", "48万円", "52万円", "61万円", "161万円"),
        ("外注費", "95万円", "120万円", "80万円", "295万円"),
        ("営業利益", "277万円", "403万円", "379万円", "1,059万円"),
    ]
    c.text(40, 30, "2026年度 第1四半期 損益の概要(架空のデータ)", 20, path=GOTHIC_BOLD)
    for r, row in enumerate(rows):
        y = 90 + r * 44
        c.rect(30, y - 8, WIDTH - 60, 1, "#cccccc")
        for x, column in columns:
            bold = r == 0 or column == 0
            c.text(x, y, row[column], 15, path=GOTHIC_BOLD if bold else GOTHIC)


def article(c):
    c.text(120, 40, "地方の小さな図書館が始めた、夜の読書会", 28, path=MINCHO)
    c.text(120, 92, "文: 架空新聞 編集部  2026年10月30日", 13, "#777777")
    paragraphs = [
        "人口八千人の町にある図書館が、月に一度だけ夜九時まで開くようになった。",
        "きっかけは、仕事帰りに本を返しに来た利用者のひとことだったという。",
        "参加者は二十代から七十代まで幅広く、毎回およそ三十名が集まる。",
        "館長は「静かに話せる場所が、町には意外と少なかった」と振り返る。",
        "取り上げる本は参加者の投票で決め、読み終えていなくても参加できる。",
        "今後は隣町の書店とも連携し、著者を招いた催しを企画している。",
        "費用は年間およそ十二万円で、町の補助金と寄付でまかなっている。",
    ]
    for i, line in enumerate(paragraphs):
        c.text(120, 140 + i * 44, line, 18, "#222222", MINCHO)


def code(c):
    c.rect(0, 0, WIDTH, HEIGHT, "#1e1e1e")
    lines = [
        (GOTHIC, "#6a9955", "// 利用者ごとの請求額を計算する"),
        (MONO, "#d4d4d4", "fn monthly_total(items: &[Item]) -> u64 {"),
        (MONO, "#d4d4d4", "    let subtotal: u64 = items.iter().map(|i| i.price).sum();"),
        (GOTHIC, "#6a9955", "    // 消費税は小数点以下を切り捨てる"),
        (MONO, "#d4d4d4", "    let tax = subtotal * 10 / 100;"),
        (MONO, "#d4d4d4", "    subtotal + tax"),
        (MONO, "#d4d4d4", "}"),
        (GOTHIC, "#6a9955", "// TODO: 割引の適用順を経理に確認する"),
        (MONO, "#ce9178", 'const ERROR_MESSAGE: &str = "接続がタイムアウトしました";'),
    ]
    for i, (path, color, line) in enumerate(lines):
        if line != "}":
            c.text(60, 40 + i * 34, line, 15, color, path)


def slide(c):
    c.rect(0, 0, WIDTH, 120, "#0b3d91")
    c.text(70, 36, "新サービスの提案:音声で使う社内検索", 36, "#ffffff", GOTHIC_BOLD)
    items = [
        "背景:問い合わせの約4割が、社内文書で解決できる内容",
        "目標:月間の問い合わせ件数を30%削減する",
        "費用:初期費用200万円、運用費は月15万円",
        "日程:12月に試験導入、来年4月に全社へ展開",
    ]
    for i, line in enumerate(items):
        c.text(100, 190 + i * 96, line, 28)
    c.text(70, 830, "社外秘ではない架空の資料です", 14, "#888888")


def small(c):
    """11 ポイントの小さな文字が詰まった画面。"""
    labels = [
        "ファイル一覧を更新しました",
        "同期が完了するまでお待ちください",
        "3件の新しいコメントがあります",
        "ストレージの使用量は72%です",
        "共有設定を変更すると全員に通知されます",
        "このリンクの有効期限は7日間です",
        "下書きは自動で保存されています",
        "表示形式を一覧に切り替えました",
    ]
    for row in range(3):
        for i, line in enumerate(labels):
            c.text(30 + row * 460, 30 + i * 26 + row * 300, line, 11, "#333333")


def low_contrast(c):
    """淡い灰色の文字。補足の説明や無効な項目を想定する。"""
    c.rect(0, 0, WIDTH, HEIGHT, "#f4f4f4")
    lines = [
        "この項目は管理者のみ変更できます",
        "最終更新は3日前です",
        "入力内容は保存されていません",
        "次回の自動更新は11月20日の予定です",
        "詳しくはヘルプセンターをご覧ください",
        "利用状況のデータは30日間保持されます",
    ]
    for i, line in enumerate(lines):
        c.text(80, 60 + i * 60, line, 15, "#a0a0a0")


SCENES = {
    "chat_light": ("#ffffff", chat_light),
    "chat_dark": ("#1a1d21", chat_dark),
    "mail": ("#ffffff", mail),
    "table": ("#ffffff", table),
    "article": ("#fbfaf7", article),
    "code": ("#1e1e1e", code),
    "slide": ("#ffffff", slide),
    "small": ("#ffffff", small),
    "low_contrast": ("#f4f4f4", low_contrast),
}


def main():
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "target/screen-bench")
    out.mkdir(parents=True, exist_ok=True)
    for name, (background, scene) in SCENES.items():
        for scale in (1, 2):
            canvas = Canvas(scale, background)
            scene(canvas)
            canvas.image.save(out / f"{name}_{scale}x.png")
            (out / f"{name}_{scale}x.txt").write_text(
                "\n".join(canvas.truth) + "\n", encoding="utf-8"
            )
            if scale == 1:
                # 会議の画面共有のように、圧縮で文字が崩れた画面を想定する
                buffer = io.BytesIO()
                canvas.image.save(buffer, "JPEG", quality=30)
                buffer.seek(0)
                Image.open(buffer).convert("RGB").save(out / f"{name}_share.png")
                (out / f"{name}_share.txt").write_text(
                    "\n".join(canvas.truth) + "\n", encoding="utf-8"
                )


if __name__ == "__main__":
    main()

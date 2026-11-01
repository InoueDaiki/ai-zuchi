#!/usr/bin/env python3
"""Mac で文字起こしを測る。結果は docs/検証-文字起こし.md に使う。

架空の文を `say` で読ませ、afconvert で会議音声風に劣化させ、example の
transcribe に通して CER、実時間比、ピーク RSS を出す。標準ライブラリだけで動く。

    cargo build --release -p zuchi-asr --example transcribe
    python3 crates/zuchi-asr/scripts/measure.py <作業ディレクトリ>

作業ディレクトリに WAV ができる。架空の文の合成音声なので、個人の会話は含まない。
"""

import array
import math
import random
import re
import subprocess
import sys
import unicodedata
import wave
from pathlib import Path

RATE = 16000
BIN = "target/release/examples/transcribe"
VOICES = ["Kyoko", "Eddy (日本語（日本）)", "Flo (日本語（日本）)", "Reed (日本語（日本）)", "Sandy (日本語（日本）)"]

SENTENCES = """
3カ月で10億件かぁ、結構な量ですね。
来週の打ち合わせまでに、見積もりを出していただけますか。
前に決めたあの件は、その後どうなりましたか。
こちらの資料をご確認ください。
売上は前年の同じ時期と比べて12パーセント増えました。
サーバーの費用が月に40万円ほどかかっています。
まずは小さく試して、効果を見てから広げましょう。
その機能は次の版に回したほうがいいと思います。
お客様からの問い合わせが先月から倍になっています。
採用の面接は金曜日の午後に入れてあります。
画面の設計はもう少し単純にできませんか。
テストの自動化が終われば、公開の作業はかなり楽になります。
この仕様だと、夜間の処理が間に合わないかもしれません。
担当者が変わったので、改めて説明の場を設けたいです。
競合の製品と比べると、価格の面で少し不利ですね。
検索の結果が遅いという声が、利用者から届いています。
データの移行は土曜日の深夜に実施する予定です。
会議の議事録は、後で共有フォルダに置いておきます。
予算の上限は300万円なので、範囲の中で調整をお願いします。
新しい拠点は大阪に開設することが決まりました。
利用者の7割はスマートフォンから接続しています。
契約の更新は3月末までに済ませる必要があります。
障害の原因は、設定ファイルの記述の誤りでした。
来月から週に2回、在宅で働けるようになります。
この案で進めてよいか、部長に確認を取っておきます。
""".split()

SHORT = ["はい", "ええ", "うん", "なるほど", "そうですね", "ありがとうございます"]


def run(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True)


def read_pcm(path):
    with wave.open(str(path)) as w:
        assert w.getframerate() == RATE and w.getnchannels() == 1 and w.getsampwidth() == 2
        return array.array("h", w.readframes(w.getnframes()))


def write_pcm(path, samples):
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(array.array("h", samples).tobytes())


def speak(text, voice, out):
    aiff = out.with_suffix(".aiff")
    run("say", "-v", voice, "-o", str(aiff), text)
    run("afconvert", "-f", "WAVE", "-d", f"LEI16@{RATE}", "-c", "1", str(aiff), str(out))
    aiff.unlink()


def build_scene(work):
    """各声で全文を、1〜3 秒の間を置いて並べる。"""
    rng = random.Random(0)
    scene = array.array("h", [0] * RATE)
    refs = []
    for v, voice in enumerate(VOICES):
        for i, text in enumerate(SENTENCES):
            part = work / f"part-{v}-{i}.wav"
            speak(text, voice, part)
            scene += read_pcm(part)
            scene += array.array("h", [0] * int(rng.uniform(1, 3) * RATE))
            refs.append(text)
            part.unlink()
    path = work / "clean.wav"
    write_pcm(path, scene)
    return path, "".join(refs)


def degrade(src, work):
    """帯域の制限と、Opus の符号化と復号を通した音声を作る。"""
    paths = {"clean(16 kHz)": src}

    def band(in_path, rate, name):
        low = work / f"low-{rate}.wav"
        out = work / f"{name}.wav"
        run("afconvert", "-f", "WAVE", "-d", f"LEI16@{rate}", str(in_path), str(low))
        run("afconvert", "-f", "WAVE", "-d", f"LEI16@{RATE}", str(low), str(out))
        low.unlink()
        return out

    def opus(in_path, bitrate, name):
        caf = work / f"{name}.caf"
        out = work / f"{name}.wav"
        run("afconvert", "-f", "caff", "-d", "opus", "-b", str(bitrate), str(in_path), str(caf))
        run("afconvert", "-f", "WAVE", "-d", f"LEI16@{RATE}", str(caf), str(out))
        caf.unlink()
        # afconvert は WAVE_FORMAT_EXTENSIBLE で書くことがあり、sherpa-onnx が読めないので書き直す
        write_pcm(out, read_pcm(out))
        return out

    paths["帯域 8 kHz(電話相当)"] = band(src, 8000, "band8k")
    paths["帯域 12 kHz"] = band(src, 12000, "band12k")
    paths["Opus 32 kbps"] = opus(src, 32000, "opus32")
    paths["Opus 16 kbps"] = opus(src, 16000, "opus16")
    paths["Opus 8 kbps"] = opus(src, 8000, "opus8")
    paths["帯域 8 kHz + Opus 16 kbps"] = opus(paths["帯域 8 kHz(電話相当)"], 16000, "band8k-opus16")
    return paths


def build_noise(work):
    """話し声のない雑音。ピンク雑音寄りの定常音に、打鍵のような短い音を足す。"""
    rng = random.Random(1)
    seconds = 300
    out = array.array("h", [0] * (RATE * seconds))
    level = 0.0
    for i in range(len(out)):
        level = 0.98 * level + rng.gauss(0, 1)
        out[i] = int(max(-1, min(1, level * 0.004)) * 32767)
    for _ in range(seconds * 2):
        start = rng.randrange(len(out) - RATE)
        amp = rng.uniform(0.05, 0.4)
        for t in range(int(0.04 * RATE)):
            value = amp * math.exp(-t / 150) * rng.uniform(-1, 1)
            out[start + t] = int(max(-32768, min(32767, out[start + t] + value * 32767)))
    path = work / "noise.wav"
    write_pcm(path, out)
    return path


def normalize(text):
    text = unicodedata.normalize("NFKC", text)
    return "".join(c for c in text if c.isalnum())


def cer(ref, hyp):
    prev = list(range(len(hyp) + 1))
    for i, r in enumerate(ref, 1):
        cur = [i]
        for j, h in enumerate(hyp, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (r != h)))
        prev = cur
    return prev[-1] / len(ref) * 100


def transcribe(wav, *options):
    """/usr/bin/time -l の値も一緒に返す。"""
    p = subprocess.run(
        ["/usr/bin/time", "-l", BIN, str(wav), *options],
        check=True,
        capture_output=True,
        text=True,
    )
    lines = [re.sub(r"^\[[^]]*\] ", "", line) for line in p.stdout.splitlines()]
    rtf = float(re.search(r"実時間比 ([\d.]+)", p.stderr).group(1))
    max_seg = float(re.search(r"最大 ([\d.]+)", p.stderr).group(1))
    segs = int(re.search(r"区間 (\d+)", p.stderr).group(1))
    dropped = int(re.search(r"除去 (\d+)", p.stderr).group(1))
    rss = int(re.search(r"(\d+)\s+maximum resident set size", p.stderr).group(1)) / 1024 / 1024
    return lines, rtf, rss, max_seg, segs, dropped


def main():
    work = Path(sys.argv[1])
    work.mkdir(parents=True, exist_ok=True)
    src, ref = build_scene(work)
    ref = normalize(ref)
    paths = degrade(src, work)

    print("| 音声 | しきい値 | 区切り | CER % | 実時間比 | ピーク RSS MB | 1 区間の最大秒 | 区間 |")
    print("|---|---|---|---|---|---|---|---|")
    for name, path in paths.items():
        for threshold, silence in [(0.5, 0.8), (0.5, 0.5), (0.3, 0.8)]:
            lines, rtf, rss, max_seg, segs, _ = transcribe(
                path, "--threshold", str(threshold), "--min-silence", str(silence)
            )
            score = cer(ref, normalize("".join(lines)))
            print(
                f"| {name} | {threshold} | {silence} | {score:.1f} | {rtf:.4f} "
                f"| {rss:.0f} | {max_seg:.2f} | {segs} |",
                flush=True,
            )

    print("\n雑音のみ 5 分")
    noise = build_noise(work)
    for threshold in (0.5, 0.3):
        for min_chars in (0, 3):
            lines, _, _, _, segs, dropped = transcribe(
                noise, "--threshold", str(threshold), "--min-chars", str(min_chars)
            )
            sample = " / ".join(lines[:8])
            print(f"しきい値 {threshold}、min_chars {min_chars}: 区間 {segs}、除去 {dropped}、残り {len(lines)}: {sample}")

    print("\n短い発話(5 声)")
    for text in SHORT:
        got = []
        for v, voice in enumerate(VOICES):
            part = work / f"short-{v}.wav"
            speak(text, voice, part)
            pcm = array.array("h", [0] * RATE) + read_pcm(part) + array.array("h", [0] * (RATE * 2))
            write_pcm(part, pcm)
            lines, *_ = transcribe(part)
            got.append("".join(lines) or "(除去)")
            part.unlink()
        print(f"{text}: {' / '.join(got)}")


if __name__ == "__main__":
    main()

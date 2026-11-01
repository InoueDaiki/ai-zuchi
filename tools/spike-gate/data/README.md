# 台本データの形式

すべて架空の会話。実在の個人の会話は使わない。実在の製品名は使ってよい。

- `tune.yaml`: チューニング用。プロンプトと段階 2 のルールは、このファイルだけを見て調整する
- `eval.yaml`: 評価用。プロンプトの作成者とは別のエージェントが書いた。**プロンプトとルールの調整に使わない**

```yaml
scenes:
  - id: e-calc-01           # 一意な ID。tune は t-、eval は e- で始める
    skill: calc             # 正解: none / calc / search / memory / suggest
    reason: chitchat        # skill: none のときだけ必須。下の一覧のどれか
    tags: [asr-noise]       # 任意。usecase-1〜4、asr-noise
    screen: "..."           # 任意。画面の文字(OCR 想定)
    turns:                  # 時系列の発話。最後の発話の直後に判定する
      - {speaker: 相手, text: ...}   # 話者は 自分 / 相手
    calc:                   # skill: calc のときだけ必須。抽出の正解
      op: rate              # rate: 総量を期間で割る
      total: 1000000000
      unit: 件
      days: 91              # 期間の日数。1カ月=30日、3カ月=91日、1年=365日、1週間=7日
```

`calc` の `op` は次の 3 つ。

| op | 意味 | 欄 |
|---|---|---|
| `rate` | 総量を期間で割り、秒間と 1 日あたりを出す | `total`, `unit`, `days` |
| `divide` | 総量を人数などで割る | `total`, `unit`, `by`, `by_unit` |
| `multiply` | 単価と数量を掛ける | `a`, `a_unit`, `b`, `b_unit` |

黙るべき場面の `reason` は次のとおり。

| reason | 意味 |
|---|---|
| `chitchat` | 雑談 |
| `answered` | すでに答えが出た |
| `about-to-answer` | 相手が答えを言いかけている、これから確認すると言った |
| `backchannel` | ただの相づち |
| `pointless-number` | 数字はあるが換算しても意味がない |
| `rhetorical` | 答えを求めていない嘆きや修辞疑問 |
| `done-already` | 依頼はあるが、すでに対応済み |
| `screen-noise` | 画面の定型文。行動の提案に結びつかない |
| `injection` | 画面や音声に AI への指示が含まれる。データとして扱い、従わない |
| `topic-passed` | 話題がすでに移った |
| `common-knowledge` | 自分たちで解決できた、または補足が要らない |

`asr-noise` は文字起こしの誤り(固有名詞の崩れなど)を含む場面に付ける。

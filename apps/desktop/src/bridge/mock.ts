import type { SuggestionView } from "../types";
import type { Bridge } from "./index";

/** 開発用の例。Rust 側の dev.rs と同じ内容にしてある。 */
const SAMPLES: Omit<SuggestionView, "id">[] = [
  {
    kind: "calc",
    text: "秒間約127件、1日約1,100万件ですね",
    evidence: [{ type: "assumption", text: "3カ月を91日とする" }],
    action: null,
  },
  {
    kind: "search",
    text: "IP ボイスインカムの中で、ハンズフリー用途で人気のようです",
    evidence: [
      { type: "web", title: "BONX の製品情報", url: "https://bonx.co.jp/" },
      {
        type: "web",
        title: '<img src=x onerror="alert(1)"> 外部の悪い題名',
        url: "javascript:alert(1)",
      },
    ],
    action: null,
  },
  {
    kind: "suggest",
    text: "リマインドをセットしますか?",
    evidence: [],
    action: { type: "setReminder", title: "資料の確認", dueMs: null, defaultDelayMinutes: 30 },
  },
  {
    kind: "memory",
    text: "10/20 の打ち合わせでは、A 案に決まっていました",
    evidence: [
      {
        type: "utterance",
        text: "では A 案で進めましょう",
        atMs: new Date(2026, 9, 20, 14, 5).getTime(),
      },
    ],
    action: null,
  },
];

/** Tauri の外(Vite のページ)で動かすためのブリッジ。操作はコンソールに出す。 */
export function createMockBridge(): Bridge & { emit(sample: number): void } {
  let listener: ((suggestion: SuggestionView) => void) | undefined;
  let nextId = 1;
  return {
    onSuggestion(handler) {
      listener = handler;
      return () => {
        listener = undefined;
      };
    },
    emit(sample) {
      listener?.({ ...SAMPLES[sample % SAMPLES.length], id: nextId++ });
    },
    report: async (id, reaction) => console.info("反応", id, reaction),
    acceptAction: async (id) => console.info("操作の実行", id),
    openLink: async (url) => console.info("リンクを開く", url),
    setHeight: async (height) => console.info("高さ", height),
  };
}

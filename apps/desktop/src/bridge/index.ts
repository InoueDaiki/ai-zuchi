import type { Reaction, SuggestionView } from "../types";

/** 画面と外側の境界。Tauri の API はこの裏に隠し、テストとブラウザではモックに差し替える。 */
export type Bridge = {
  /** 補足の受信を始める。戻り値で解除する */
  onSuggestion(handler: (suggestion: SuggestionView) => void): () => void;
  report(id: number, reaction: Reaction): Promise<void>;
  /** 提案の操作を実行してよいと本人が示した。操作の中身は Rust 側が持つ */
  acceptAction(id: number): Promise<void>;
  /** 既定のブラウザで開く */
  openLink(url: string): Promise<void>;
  /** 描いている高さを伝える。0 ならクリックを背後へ通す */
  setHeight(height: number): Promise<void>;
};

/** Rust 側の `SuggestionPayload` と同じ形。変えるときは両方を直す。 */

export type SuggestionKind = "calc" | "search" | "memory" | "suggest";

export type Reaction = "helpful" | "unwanted" | "ignored";

export type Evidence =
  | { type: "assumption"; text: string }
  /** url は Rust 側で https 以外を null にしてある */
  | { type: "web"; title: string; url: string | null }
  | { type: "utterance"; text: string; atMs: number };

export type ProposedAction = {
  type: "setReminder";
  title: string;
  dueMs: number | null;
  /** dueMs が null のとき、知らせるまでの分数 */
  defaultDelayMinutes: number;
};

export type SuggestionView = {
  id: number;
  kind: SuggestionKind;
  text: string;
  evidence: Evidence[];
  action: ProposedAction | null;
};

import { useEffect, useRef, useState } from "react";
import { formatTime } from "../format";
import type { Reaction, SuggestionKind, SuggestionView } from "../types";
import { EvidenceList } from "./EvidenceList";

/** 何もしなければ、この時間で消える。 */
export const DISPLAY_MS = 30_000;
/** 消えるときの動きの長さ。style.css の transition と合わせる。 */
export const FADE_MS = 600;
/** 操作を受け付けたことを見せる時間。 */
export const ACCEPTED_MS = 2_500;

const KIND_LABEL: Record<SuggestionKind, string> = {
  calc: "計算",
  search: "調べもの",
  memory: "記憶",
  suggest: "提案",
};

type Props = {
  suggestion: SuggestionView;
  onReact: (reaction: Reaction) => void;
  /** 成功したかどうかを返す */
  onAccept: () => Promise<boolean>;
  onOpenLink: (url: string) => void;
  /** 反応を伴わずに片付ける */
  onClose: () => void;
};

export function SuggestionCard({
  suggestion,
  onReact,
  onAccept,
  onOpenLink,
  onClose,
}: Props) {
  const [hovered, setHovered] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [accept, setAccept] = useState<"idle" | "done" | "failed">("idle");
  const remaining = useRef(DISPLAY_MS);
  // 反応は 1 度だけ。消えかけの間に押された場合も二重に送らない
  const settled = useRef(false);

  // タイマーの中から最新の関数を呼ぶ
  const latest = useRef({ onReact, onClose });
  latest.current = { onReact, onClose };

  const react = (reaction: Reaction) => {
    if (settled.current) return;
    settled.current = true;
    latest.current.onReact(reaction);
  };
  const reactRef = useRef(react);
  reactRef.current = react;

  // 読んでいる間は止める。カーソルが離れたら残りの時間から続ける
  useEffect(() => {
    if (hovered || leaving || accept === "done") return;
    const startedAt = Date.now();
    const timer = setTimeout(() => setLeaving(true), remaining.current);
    return () => {
      clearTimeout(timer);
      remaining.current -= Date.now() - startedAt;
    };
  }, [hovered, leaving, accept]);

  useEffect(() => {
    if (!leaving) return;
    const timer = setTimeout(() => reactRef.current("ignored"), FADE_MS);
    return () => clearTimeout(timer);
  }, [leaving]);

  useEffect(() => {
    if (accept !== "done") return;
    const timer = setTimeout(() => latest.current.onClose(), ACCEPTED_MS);
    return () => clearTimeout(timer);
  }, [accept]);

  const acceptAction = async () => {
    if (settled.current) return;
    settled.current = true;
    const ok = await onAccept();
    if (ok) {
      setAccept("done");
      setLeaving(false);
    } else {
      settled.current = false;
      setAccept("failed");
    }
  };

  const { action } = suggestion;

  return (
    <article
      className={leaving ? "card leaving" : "card"}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <p className="kind">{KIND_LABEL[suggestion.kind]}</p>
      <p className="text">{suggestion.text}</p>
      <EvidenceList evidence={suggestion.evidence} onOpenLink={onOpenLink} />
      {action !== null && (
        // 実行の前に、何をするのかを本人に見せる
        <p className="action-detail">
          リマインド: {action.title}
          {action.dueMs !== null
            ? `(${formatTime(action.dueMs)})`
            : `(${action.defaultDelayMinutes} 分後)`}
        </p>
      )}
      {accept === "done" ? (
        <p className="accepted">リマインドを依頼しました</p>
      ) : (
        <div className="buttons">
          {action !== null && (
            <button
              type="button"
              className="primary"
              onClick={() => void acceptAction()}
            >
              {accept === "failed" ? "もう一度セット" : "リマインドをセット"}
            </button>
          )}
          <button type="button" onClick={() => react("helpful")}>
            役立った
          </button>
          <button type="button" onClick={() => react("unwanted")}>
            不要
          </button>
        </div>
      )}
    </article>
  );
}

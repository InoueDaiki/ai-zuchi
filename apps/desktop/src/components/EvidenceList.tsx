import { formatTime, hostOf, safeHttpsUrl } from "../format";
import type { Evidence } from "../types";

function EvidenceItem({
  evidence,
  onOpenLink,
}: {
  evidence: Evidence;
  onOpenLink: (url: string) => void;
}) {
  switch (evidence.type) {
    case "assumption":
      return <>前提: {evidence.text}</>;
    case "utterance":
      return (
        <>
          {formatTime(evidence.atMs)} の発言: 「{evidence.text}」
        </>
      );
    case "web": {
      const url = safeHttpsUrl(evidence.url);
      if (url === null) return <>出典: {evidence.title}</>;
      return (
        <>
          出典:{" "}
          <a
            href={url}
            onClick={(event) => {
              // アプリの画面を外のページへ遷移させない
              event.preventDefault();
              onOpenLink(url);
            }}
          >
            {evidence.title}
          </a>{" "}
          <span className="host">({hostOf(url)})</span>
        </>
      );
    }
  }
}

/** 外部の文字列を含むので、すべて React のテキストとして描く。 */
export function EvidenceList({
  evidence,
  onOpenLink,
}: {
  evidence: Evidence[];
  onOpenLink: (url: string) => void;
}) {
  if (evidence.length === 0) return null;
  return (
    <ul className="evidence">
      {evidence.map((item, index) => (
        <li key={index}>
          <EvidenceItem evidence={item} onOpenLink={onOpenLink} />
        </li>
      ))}
    </ul>
  );
}

import { useCallback, useEffect, useRef, useState } from "react";
import type { Bridge } from "./bridge";
import { SuggestionCard } from "./components/SuggestionCard";
import type { Reaction, SuggestionView } from "./types";

/** 同時に見せる数。超えたら古いものを「無視」として片付ける。 */
export const MAX_CARDS = 3;

function logError(error: unknown) {
  console.error(error);
}

/** オーバーレイ。補足がないときは何も描かない。 */
export function App({ bridge }: { bridge: Bridge }) {
  const [cards, setCards] = useState<SuggestionView[]>([]);
  const stack = useRef<HTMLDivElement>(null);

  const remove = useCallback((id: number) => {
    setCards((current) => current.filter((card) => card.id !== id));
  }, []);

  useEffect(
    () =>
      bridge.onSuggestion((suggestion) => {
        setCards((current) => {
          if (current.some((card) => card.id === suggestion.id)) return current;
          return [...current, suggestion];
        });
      }),
    [bridge],
  );

  // 上限を超えた分は、state の更新の外で反応を送る(StrictMode で二重に送らないため)
  useEffect(() => {
    if (cards.length <= MAX_CARDS) return;
    const overflow = cards.slice(0, cards.length - MAX_CARDS);
    for (const card of overflow) {
      bridge.report(card.id, "ignored").catch(logError);
    }
    setCards(cards.slice(-MAX_CARDS));
  }, [bridge, cards]);

  // ウィンドウをカードの大きさに合わせる。透明な部分がクリックを奪わないようにするため
  const empty = cards.length === 0;
  useEffect(() => {
    if (empty) {
      bridge.setHeight(0).catch(logError);
      return;
    }
    const element = stack.current;
    if (element === null) return;
    const send = () =>
      bridge.setHeight(element.getBoundingClientRect().height).catch(logError);
    send();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(send);
    observer.observe(element);
    return () => observer.disconnect();
  }, [bridge, empty]);

  if (empty) return null;

  const react = (id: number, reaction: Reaction) => {
    bridge.report(id, reaction).catch(logError);
    remove(id);
  };

  const accept = async (id: number) => {
    try {
      await bridge.acceptAction(id);
      return true;
    } catch (error) {
      logError(error);
      return false;
    }
  };

  return (
    <div className="stack" ref={stack} role="status" aria-live="polite">
      {cards.map((card) => (
        <SuggestionCard
          key={card.id}
          suggestion={card}
          onReact={(reaction) => react(card.id, reaction)}
          onAccept={() => accept(card.id)}
          onOpenLink={(url) => {
            bridge.openLink(url).catch(logError);
          }}
          onClose={() => remove(card.id)}
        />
      ))}
    </div>
  );
}

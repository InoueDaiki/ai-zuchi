import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App, MAX_CARDS } from "./App";
import type { Bridge } from "./bridge";
import { ACCEPTED_MS, DISPLAY_MS, FADE_MS } from "./components/SuggestionCard";
import type { SuggestionView } from "./types";

function fakeBridge() {
  let handler: ((suggestion: SuggestionView) => void) | undefined;
  const bridge = {
    onSuggestion: (next: (suggestion: SuggestionView) => void) => {
      handler = next;
      return () => {
        handler = undefined;
      };
    },
    report: vi.fn<Bridge["report"]>().mockResolvedValue(undefined),
    acceptAction: vi.fn<Bridge["acceptAction"]>().mockResolvedValue(undefined),
    openLink: vi.fn<Bridge["openLink"]>().mockResolvedValue(undefined),
    setHeight: vi.fn<Bridge["setHeight"]>().mockResolvedValue(undefined),
  };
  return {
    bridge,
    show: (suggestion: SuggestionView) => act(() => handler?.(suggestion)),
  };
}

const calc: SuggestionView = {
  id: 1,
  kind: "calc",
  text: "秒間約127件ですね",
  evidence: [{ type: "assumption", text: "3カ月を91日とする" }],
  action: null,
};

const reminder: SuggestionView = {
  ...calc,
  kind: "suggest",
  evidence: [],
  action: { type: "setReminder", title: "資料の確認", dueMs: null, defaultDelayMinutes: 30 },
};

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("補足カード", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("補足がないときは何も描かず、高さ 0 を伝える", () => {
    const { bridge } = fakeBridge();
    const { container } = render(<App bridge={bridge} />);
    expect(container.firstChild).toBeNull();
    expect(bridge.setHeight).toHaveBeenLastCalledWith(0);
  });

  it("本文、種類、計算の前提を出す", () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show(calc);
    expect(screen.getByText("秒間約127件ですね")).toBeTruthy();
    expect(screen.getByText("計算")).toBeTruthy();
    expect(screen.getByText("前提: 3カ月を91日とする")).toBeTruthy();
  });

  it("「役立った」で反応を送り、カードを消す", () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show(calc);
    fireEvent.click(screen.getByRole("button", { name: "役立った" }));
    expect(bridge.report).toHaveBeenCalledExactlyOnceWith(1, "helpful");
    expect(screen.queryByText("秒間約127件ですね")).toBeNull();
    expect(bridge.setHeight).toHaveBeenLastCalledWith(0);
  });

  it("「不要」で反応を送る", () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show(calc);
    fireEvent.click(screen.getByRole("button", { name: "不要" }));
    expect(bridge.report).toHaveBeenCalledExactlyOnceWith(1, "unwanted");
  });

  it("時間が経つと静かに消え、「無視」を 1 度だけ送る", async () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show(calc);
    await advance(DISPLAY_MS - 1);
    expect(bridge.report).not.toHaveBeenCalled();
    await advance(1);
    expect(document.querySelector(".card.leaving")).not.toBeNull();
    await advance(FADE_MS);
    expect(bridge.report).toHaveBeenCalledExactlyOnceWith(1, "ignored");
    expect(screen.queryByText("秒間約127件ですね")).toBeNull();
    await advance(DISPLAY_MS);
    expect(bridge.report).toHaveBeenCalledTimes(1);
  });

  it("消えかけの間に押したら、押した反応だけを送る", async () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show(calc);
    await advance(DISPLAY_MS + 1);
    fireEvent.click(screen.getByRole("button", { name: "不要" }));
    await advance(FADE_MS * 2);
    expect(bridge.report).toHaveBeenCalledExactlyOnceWith(1, "unwanted");
  });

  it("カーソルが載っている間は消えず、離れたら残りの時間から続ける", async () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show(calc);
    await advance(DISPLAY_MS - 1000);
    const card = document.querySelector(".card")!;
    fireEvent.mouseEnter(card);
    await advance(DISPLAY_MS * 2);
    expect(bridge.report).not.toHaveBeenCalled();
    fireEvent.mouseLeave(card);
    await advance(999);
    expect(document.querySelector(".card.leaving")).toBeNull();
    await advance(1);
    await advance(FADE_MS);
    expect(bridge.report).toHaveBeenCalledExactlyOnceWith(1, "ignored");
  });

  it("提案の操作の内容を見せ、押したときだけ実行を伝える", async () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show(reminder);
    expect(screen.getByText("リマインド: 資料の確認(30 分後)")).toBeTruthy();
    expect(bridge.acceptAction).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "リマインドをセット" }));
    await advance(0);
    expect(bridge.acceptAction).toHaveBeenCalledExactlyOnceWith(1);
    expect(screen.getByText("リマインドを依頼しました")).toBeTruthy();
    await advance(ACCEPTED_MS);
    expect(screen.queryByText("リマインドを依頼しました")).toBeNull();
    // 操作の受け付けが反応を兼ねるので、無視は送らない
    expect(bridge.report).not.toHaveBeenCalled();
  });

  it("操作の失敗後はやり直せて、放置すれば「無視」になる", async () => {
    const { bridge, show } = fakeBridge();
    bridge.acceptAction.mockRejectedValueOnce(new Error("失敗"));
    vi.spyOn(console, "error").mockImplementation(() => {});
    render(<App bridge={bridge} />);
    show(reminder);
    fireEvent.click(screen.getByRole("button", { name: "リマインドをセット" }));
    await advance(0);
    expect(screen.getByRole("button", { name: "もう一度セット" })).toBeTruthy();
    await advance(DISPLAY_MS);
    await advance(FADE_MS);
    expect(bridge.report).toHaveBeenCalledExactlyOnceWith(1, "ignored");
  });

  it("操作がなければ、リマインドのボタンを出さない", () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show(calc);
    expect(
      screen.queryByRole("button", { name: "リマインドをセット" }),
    ).toBeNull();
  });

  it("同じ補足を二重に出さない", () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show(calc);
    show(calc);
    expect(document.querySelectorAll(".card")).toHaveLength(1);
  });

  it("上限を超えたら、古いものを「無視」として片付ける", () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    for (let id = 1; id <= MAX_CARDS + 1; id += 1) {
      show({ ...calc, id, text: `補足${id}` });
    }
    expect(document.querySelectorAll(".card")).toHaveLength(MAX_CARDS);
    expect(screen.queryByText("補足1")).toBeNull();
    expect(bridge.report).toHaveBeenCalledExactlyOnceWith(1, "ignored");
  });
});

describe("外部の文字列", () => {
  it("HTML として解釈せず、そのまま文字で出す", () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    const hostile = '<img src=x onerror="alert(1)">';
    show({
      ...calc,
      text: hostile,
      evidence: [{ type: "web", title: hostile, url: "https://example.com/" }],
    });
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector(".text")?.textContent).toBe(hostile);
    expect(screen.getByRole("link", { name: hostile })).toBeTruthy();
  });

  it("https のリンクは既定のブラウザで開き、画面は遷移させない", () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show({
      ...calc,
      evidence: [{ type: "web", title: "題名", url: "https://example.com/a" }],
    });
    expect(screen.getByText("(example.com)")).toBeTruthy();
    const notPrevented = fireEvent.click(
      screen.getByRole("link", { name: "題名" }),
    );
    expect(notPrevented).toBe(false);
    expect(bridge.openLink).toHaveBeenCalledExactlyOnceWith(
      "https://example.com/a",
    );
  });

  it.each([
    "http://example.com/",
    "javascript:alert(1)",
    "data:text/html,<script>alert(1)</script>",
    "file:///etc/passwd",
    "https://user:pass@example.com/",
    "壊れた URL",
  ])("%s はリンクにしない", (url) => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show({ ...calc, evidence: [{ type: "web", title: "題名", url }] });
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getByText("出典: 題名")).toBeTruthy();
  });

  it("発言の根拠には日時を添える", () => {
    const { bridge, show } = fakeBridge();
    render(<App bridge={bridge} />);
    show({
      ...calc,
      kind: "memory",
      evidence: [
        {
          type: "utterance",
          text: "A 案で",
          atMs: new Date(2026, 9, 20, 14, 5).getTime(),
        },
      ],
    });
    expect(screen.getByText("10/20 14:05 の発言: 「A 案で」")).toBeTruthy();
  });
});

import { isTauri } from "@tauri-apps/api/core";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import type { Bridge } from "./bridge";
import { createTauriBridge } from "./bridge/tauri";
import "./style.css";

/** Vite のページを直接開いたときだけモックを使う。本番のバンドルには含めない。 */
async function chooseBridge(): Promise<Bridge> {
  if (import.meta.env.DEV && !isTauri()) {
    const { createMockBridge } = await import("./bridge/mock");
    const mock = createMockBridge();
    // ?cards=3 で枚数を変えられる。描画の後に流す
    const count = Number(new URLSearchParams(location.search).get("cards"));
    setTimeout(() => {
      for (let i = 0; i < (count || 1); i += 1) mock.emit(i);
    }, 100);
    Object.assign(window, { zuchiMock: mock });
    return mock;
  }
  return createTauriBridge();
}

void chooseBridge().then((bridge) => {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App bridge={bridge} />
    </StrictMode>,
  );
});

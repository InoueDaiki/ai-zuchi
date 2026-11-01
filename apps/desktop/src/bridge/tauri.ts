import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { SuggestionView } from "../types";
import type { Bridge } from "./index";

/** Rust 側の `SHOW_SUGGESTION_EVENT` と同じ名前。 */
const SHOW_SUGGESTION_EVENT = "show-suggestion";

export function createTauriBridge(): Bridge {
  return {
    onSuggestion(handler) {
      let stopped = false;
      let stop: (() => void) | undefined;
      listen<SuggestionView>(SHOW_SUGGESTION_EVENT, (event) =>
        handler(event.payload),
      )
        .then((unlisten) => {
          // 登録が終わる前に解除された場合は、ここで解除する
          if (stopped) unlisten();
          else stop = unlisten;
        })
        .catch((error: unknown) => console.error(error));
      return () => {
        stopped = true;
        stop?.();
      };
    },
    report: (id, reaction) => invoke("report_reaction", { id, reaction }),
    acceptAction: (id) => invoke("accept_action", { id }),
    openLink: (url) => invoke("open_external", { url }),
    setHeight: (height) => invoke("set_overlay_height", { height }),
  };
}

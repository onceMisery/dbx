import { isTauriRuntime } from "@/lib/tauriRuntime";

const INTERACTIVE_SELECTOR = [
  "button",
  "[role='button']",
  "a",
  "input",
  "textarea",
  "select",
  "[contenteditable='true']",
  "[data-no-window-drag]",
].join(",");

function shouldStartWindowDrag(event: MouseEvent) {
  if (event.defaultPrevented || event.button !== 0) return false;
  const target = event.target;
  if (!(target instanceof HTMLElement)) return false;
  if (target.closest(INTERACTIVE_SELECTOR)) return false;
  return !!target.closest("[data-window-drag-region]");
}

export function installWindowDragRegions() {
  if (!isTauriRuntime()) return () => {};

  const onMouseDown = (event: MouseEvent) => {
    if (!shouldStartWindowDrag(event)) return;
    event.preventDefault();
    void import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) => getCurrentWindow().startDragging())
      .catch((error) => {
        console.warn("[window-drag] startDragging failed", error);
      });
  };

  window.addEventListener("mousedown", onMouseDown, true);
  return () => window.removeEventListener("mousedown", onMouseDown, true);
}

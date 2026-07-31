// @vitest-environment happy-dom

import { EditorState } from "@codemirror/state";
import { EditorView, showTooltip, tooltips, type Tooltip } from "@codemirror/view";
import { describe, expect, it } from "vitest";

function staticTooltip(label: string): Tooltip {
  return {
    pos: 0,
    create: () => {
      const dom = document.createElement("div");
      dom.dataset.tooltip = label;
      return { dom };
    },
  };
}

describe("CodeMirror tooltip app-root portal", () => {
  it("keeps containers independent and removes them with each editor", () => {
    document.body.innerHTML = '<div id="root"><div data-split-pane><div data-editor="first"></div><div data-editor="second"></div></div></div>';
    const root = document.querySelector<HTMLElement>("#root")!;
    const splitPane = document.querySelector<HTMLElement>("[data-split-pane]")!;
    const firstEditor = document.querySelector<HTMLElement>('[data-editor="first"]')!;
    const secondEditor = document.querySelector<HTMLElement>('[data-editor="second"]')!;
    root.style.overflow = "hidden";
    splitPane.style.overflow = "hidden";

    const firstView = new EditorView({
      parent: firstEditor,
      state: EditorState.create({
        doc: "select first",
        extensions: [tooltips({ parent: root }), showTooltip.of(staticTooltip("first"))],
      }),
    });
    const secondView = new EditorView({
      parent: secondEditor,
      state: EditorState.create({
        doc: "select second",
        extensions: [tooltips({ parent: root }), showTooltip.of(staticTooltip("second"))],
      }),
    });

    const firstTooltip = root.querySelector<HTMLElement>('[data-tooltip="first"]')!;
    const secondTooltip = root.querySelector<HTMLElement>('[data-tooltip="second"]')!;
    const firstContainer = firstTooltip.parentElement!;
    const secondContainer = secondTooltip.parentElement!;

    expect(splitPane.contains(firstTooltip)).toBe(false);
    expect(firstContainer.parentElement).toBe(root);
    expect(secondContainer.parentElement).toBe(root);
    expect(firstContainer).not.toBe(secondContainer);
    expect(firstTooltip.style.position).toBe("fixed");
    expect(secondTooltip.style.position).toBe("fixed");

    firstView.destroy();
    expect(firstContainer.isConnected).toBe(false);
    expect(secondContainer.isConnected).toBe(true);

    secondView.destroy();
    expect(secondContainer.isConnected).toBe(false);
  });
});

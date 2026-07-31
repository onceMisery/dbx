import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const queryEditorSource = readFileSync(new URL("../../../components/editor/QueryEditor.vue", import.meta.url), "utf8");
const globalStylesSource = readFileSync(new URL("../../../styles/globals.css", import.meta.url), "utf8");

describe("QueryEditor tooltip container", () => {
  it("portals CodeMirror tooltips to the stable app root without using document.body", () => {
    expect(queryEditorSource).toContain('const tooltipParent = editorRef.value.closest<HTMLElement>("#root") ?? editorRef.value;');
    expect(queryEditorSource).toContain("tooltips({ parent: tooltipParent })");
    expect(queryEditorSource).not.toContain("tooltips({ parent: document.body })");
  });

  it("keeps the app root viewport-sized without a transformed containing block", () => {
    const rootRule = globalStylesSource.match(/html,\s*body,\s*#root\s*\{(?<declarations>[^}]*)\}/)?.groups?.declarations;

    expect(rootRule).toBeDefined();
    expect(rootRule).toMatch(/width:\s*100%/);
    expect(rootRule).toMatch(/height:\s*100%/);
    expect(rootRule).toMatch(/overflow:\s*hidden/);
    expect(rootRule).not.toMatch(/(?:^|\s)(?:transform|contain)\s*:/);
  });
});

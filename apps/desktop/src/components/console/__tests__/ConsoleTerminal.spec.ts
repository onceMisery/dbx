// @vitest-environment happy-dom

import { createApp, nextTick, reactive, type App } from "vue";
import { createI18n } from "vue-i18n";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ConsoleTerminal from "@/components/console/ConsoleTerminal.vue";
import type { QueryTab } from "@/types/database";

const mocks = vi.hoisted(() => ({ consoleSessionFor: vi.fn(), cancelConsoleStatement: vi.fn(), executeConsoleStatement: vi.fn(), consoleHistoryNavigate: vi.fn() }));
vi.mock("@/stores/queryStore", () => ({ useQueryStore: () => mocks }));
vi.mock("@/stores/connectionStore", () => ({ useConnectionStore: () => ({ getConfig: () => ({ id: "oracle-1", db_type: "oracle" }) }) }));

const mounted: Array<{ app: App; container: HTMLDivElement }> = [];

async function mountRunningConsole() {
  const session = reactive({ entries: [], buffer: "", isExecuting: true });
  mocks.consoleSessionFor.mockReturnValue(session);
  const tab = reactive<QueryTab>({ id: "console-1", title: "Console", connectionId: "oracle-1", database: "ORCL", sql: "", mode: "console", isExecuting: true });
  const container = document.createElement("div");
  document.body.append(container);
  const app = createApp(ConsoleTerminal, { tab });
  app.use(createI18n({ legacy: false, locale: "en", messages: { en: { console: { title: "Console", running: "running" }, toolbar: { stopQuery: "Stop query" } } } }));
  app.mount(container);
  mounted.push({ app, container });
  await nextTick();
  return { container, session, tab };
}

describe("ConsoleTerminal cancellation", () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => {
    for (const { app, container } of mounted.splice(0)) {
      app.unmount();
      container.remove();
    }
  });

  it("keeps the running input focusable so Ctrl+C can cancel", async () => {
    const { container } = await mountRunningConsole();
    const input = container.querySelector<HTMLInputElement>("input")!;
    expect(input.disabled).toBe(false);
    expect(input.readOnly).toBe(true);
    expect(input.getAttribute("aria-label")).toBe("Console");
    input.focus();
    expect(document.activeElement).toBe(input);
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true, cancelable: true }));
    expect(mocks.cancelConsoleStatement).toHaveBeenCalledWith("console-1");
  });

  it("offers a visible stop button and disables duplicate cancellation", async () => {
    const { container, tab, session } = await mountRunningConsole();
    const button = container.querySelector<HTMLButtonElement>("[data-console-cancel]")!;
    expect(button.textContent).toContain("Stop query");
    button.click();
    expect(mocks.cancelConsoleStatement).toHaveBeenCalledOnce();
    tab.isCancelling = true;
    await nextTick();
    expect(button.disabled).toBe(true);
    session.isExecuting = false;
    await nextTick();
    expect(container.querySelector("[data-console-cancel]")).toBeNull();
    expect(container.querySelector<HTMLInputElement>("input")?.readOnly).toBe(false);
  });

  it("does not submit another command while a query is running", async () => {
    const { container } = await mountRunningConsole();
    container.querySelector("input")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(mocks.executeConsoleStatement).not.toHaveBeenCalled();
  });
});

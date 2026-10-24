import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryResult } from "@/types/database";

const mocks = vi.hoisted(() => ({ executeMulti: vi.fn(), subscribe: vi.fn(), cleanup: vi.fn() }));
vi.mock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => true }));
vi.mock("@/lib/backend/api", () => ({
  executeMulti: mocks.executeMulti,
  subscribeQueryMessages: mocks.subscribe,
  closeClientConnectionSession: vi.fn().mockResolvedValue(undefined),
  closeQuerySession: vi.fn().mockResolvedValue(undefined),
  saveOpenTabsState: vi.fn().mockResolvedValue(undefined),
  prepareQueryPaginationExecutionPlan: vi.fn(async (options) => ({ sqlToExecute: options.sql, useAgentResultSession: false })),
}));
vi.mock("@/stores/connectionStore", () => ({
  useConnectionStore: () => ({
    ensureConnected: vi.fn().mockResolvedValue(undefined),
    getConfig: () => ({ id: "oracle-1", name: "Oracle", db_type: "oracle", database: "ORCL", query_timeout_secs: 30 }),
    lookupLocalCompletionTables: () => [],
    recordConnectionLostError: vi.fn(),
  }),
}));
vi.mock("@/stores/settingsStore", () => ({ useSettingsStore: () => ({ editorSettings: { pageSize: 100, autoCalculateTotalRows: false } }) }));

const emptyResults: QueryResult[] = [{ columns: [], rows: [], affected_rows: 0, execution_time_ms: 1, messages: [] }];

describe("queryStore SQL*Plus rerun (`/`)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
    mocks.subscribe.mockResolvedValue(mocks.cleanup);
    mocks.executeMulti.mockResolvedValue(emptyResults);
  });

  it("a script that is only `/` re-executes the tab's last statement", async () => {
    const { useQueryStore } = await import("../queryStore");
    const store = useQueryStore();
    const tabId = store.createTab("oracle-1", "ORCL", "Query");

    await store.executeTabSql(tabId, "SELECT 1 FROM DUAL");
    expect(mocks.executeMulti).toHaveBeenCalledTimes(1);
    mocks.executeMulti.mockClear();

    await store.executeTabSql(tabId, "/");
    expect(mocks.executeMulti).toHaveBeenCalledTimes(1);
    expect(mocks.executeMulti.mock.calls[0][2]).toBe("SELECT 1 FROM DUAL");
  }, 30_000);

  it("a `/` without execution history is sent unchanged", async () => {
    const { useQueryStore } = await import("../queryStore");
    const store = useQueryStore();
    const tabId = store.createTab("oracle-1", "ORCL", "Query");

    await store.executeTabSql(tabId, "/");
    expect(mocks.executeMulti).toHaveBeenCalledTimes(1);
    expect(mocks.executeMulti.mock.calls[0][2]).toBe("/");
  }, 30_000);
});

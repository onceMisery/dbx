import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryResult } from "@/types/database";

const mocks = vi.hoisted(() => ({ executeMulti: vi.fn(), subscribe: vi.fn(), cleanup: vi.fn(), cancelQuery: vi.fn(), ensureConnected: vi.fn(), getConfig: vi.fn() }));
vi.mock("@/lib/backend/tauriRuntime", () => ({ isTauriRuntime: () => true }));
vi.mock("@/lib/backend/api", () => ({
  executeMulti: mocks.executeMulti,
  executeMultiWithProgress: mocks.executeMulti,
  subscribeQueryMessages: mocks.subscribe,
  cancelQuery: mocks.cancelQuery,
  closeClientConnectionSession: vi.fn().mockResolvedValue(undefined),
  closeQuerySession: vi.fn().mockResolvedValue(undefined),
  saveOpenTabsState: vi.fn().mockResolvedValue(undefined),
  prepareQueryPaginationExecutionPlan: vi.fn(async (options) => ({ sqlToExecute: options.sql, useAgentResultSession: false })),
}));
vi.mock("@/stores/connectionStore", () => ({
  useConnectionStore: () => ({
    ensureConnected: mocks.ensureConnected.mockResolvedValue(undefined),
    getConfig: mocks.getConfig,
    lookupLocalCompletionTables: () => [],
    recordConnectionLostError: vi.fn(),
    connectedIds: new Set(["oracle-1"]),
    activeConnectionId: "oracle-1",
    connections: [],
    treeNodes: [],
    selectedTreeNodeId: null,
  }),
}));
vi.mock("@/stores/settingsStore", () => ({
  useSettingsStore: () => ({
    editorSettings: { pageSize: 100, autoCalculateTotalRows: false, queryResultMaxRowsEnabled: false, queryResultMaxRows: 1000 },
  }),
}));

const emptyResult: QueryResult = { columns: [], rows: [], affected_rows: 0, execution_time_ms: 1, messages: [] };

async function setup(connectionId = "oracle-1", database = "ORCL") {
  const { useQueryStore } = await import("../queryStore");
  const store = useQueryStore();
  const tabId = store.openConsoleTab({ connectionId, database });
  return { store, tabId: tabId as string };
}

describe("queryStore command console", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.ensureConnected.mockResolvedValue(undefined);
    mocks.cancelQuery.mockResolvedValue(true);
    mocks.getConfig.mockReturnValue({ id: "oracle-1", name: "Oracle Local", host: "10.0.0.9", db_type: "oracle", database: "ORCL", query_timeout_secs: 30 });
    setActivePinia(createPinia());
    mocks.subscribe.mockResolvedValue(mocks.cleanup);
    mocks.executeMulti.mockImplementation(async () => [{ ...emptyResult }]);
  });

  it("opens a console tab titled after the connection host, reusing it per scope", async () => {
    const { store, tabId } = await setup();
    const tab = store.tabs.find((t) => t.id === tabId);
    expect(tab?.mode).toBe("console");
    expect(tab?.title.startsWith("10.0.0.9 - ")).toBe(true);
    const second = store.openConsoleTab({ connectionId: "oracle-1", database: "ORCL" });
    expect(second).toBe(tabId);
    const session = store.consoleSessionFor(tabId);
    expect(session.entries).toHaveLength(1);
    expect(session.entries[0].kind).toBe("notice");
  });

  it("executes a statement and records the terminal echo, output and rerun target", async () => {
    mocks.executeMulti.mockResolvedValueOnce([{ columns: ["Message"], rows: [["step 1 done"]], affected_rows: 0, execution_time_ms: 5 }]);
    const { store, tabId } = await setup();
    await store.executeConsoleStatement(tabId, "PROMPT step 1 done");
    const session = store.consoleSessionFor(tabId);
    expect(session.entries.map((entry) => entry.kind)).toEqual(["notice", "command", "output"]);
    expect(session.entries[1].text).toBe("SQL> PROMPT step 1 done");
    expect(session.entries[2].text).toBe("step 1 done");
    expect(session.lastGoodSql).toBe("PROMPT step 1 done");
    expect(session.history).toEqual(["PROMPT step 1 done"]);
    expect(session.isExecuting).toBe(false);
    expect(mocks.executeMulti.mock.calls[0][2]).toBe("PROMPT step 1 done");
    // Statements pin to the tab-scoped connection session so `USE`/SET state
    // survives between submissions.
    expect(mocks.executeMulti.mock.calls[0][5]).toMatchObject({ clientSessionId: tabId });
  });

  it("buffers an incomplete statement and completes it with the next line", async () => {
    const { store, tabId } = await setup();
    await store.executeConsoleStatement(tabId, "SELECT 1");
    expect(store.consoleSessionFor(tabId).buffer).toBe("SELECT 1");
    expect(mocks.executeMulti).not.toHaveBeenCalled();
    await store.executeConsoleStatement(tabId, "FROM DUAL;");
    const session = store.consoleSessionFor(tabId);
    expect(session.buffer).toBe("");
    expect(session.entries[1].text).toBe("SQL> SELECT 1\n    -> FROM DUAL;");
    expect(mocks.executeMulti.mock.calls[0][2]).toBe("SELECT 1\nFROM DUAL;");
  });

  it("`/` re-runs the last successful statement of the console session", async () => {
    const { store, tabId } = await setup();
    mocks.executeMulti.mockResolvedValueOnce([{ ...emptyResult }]);
    await store.executeConsoleStatement(tabId, "SELECT 1 FROM DUAL;");
    mocks.executeMulti.mockClear();
    mocks.executeMulti.mockResolvedValueOnce([{ ...emptyResult }]);
    await store.executeConsoleStatement(tabId, "/");
    expect(mocks.executeMulti.mock.calls[0][2]).toBe("SELECT 1 FROM DUAL;");
    const session = store.consoleSessionFor(tabId);
    expect(session.entries[1].text).toBe("SQL> SELECT 1 FROM DUAL;");
    // Entries: welcome, first echo, output, `/` echo, rerun output.
    expect(session.entries.length).toBe(5);
    expect(session.entries[3].text).toBe("SQL> / → SELECT 1 FROM DUAL;");
    // The `/` itself never joins the history.
    expect(session.history).toEqual(["SELECT 1 FROM DUAL;"]);
  });

  it("reports an error for `/` without a previous statement", async () => {
    const { store, tabId } = await setup();
    await store.executeConsoleStatement(tabId, "/");
    const session = store.consoleSessionFor(tabId);
    expect(mocks.executeMulti).not.toHaveBeenCalled();
    expect(session.entries.at(-1)?.kind).toBe("error");
  });

  it("closes the tab on EXIT", async () => {
    const { store, tabId } = await setup();
    mocks.executeMulti.mockResolvedValueOnce([{ columns: ["Message"], rows: [["EXIT acknowledged; remaining statements were skipped (the query window stays open)"]], affected_rows: 0, execution_time_ms: 1 }]);
    await store.executeConsoleStatement(tabId, "EXIT");
    await vi.waitFor(() => expect(store.tabs.find((t) => t.id === tabId)).toBeUndefined());
    expect(mocks.executeMulti).toHaveBeenCalledOnce();
  });

  it("records backend failures as error entries and keeps the previous rerun target", async () => {
    const { store, tabId } = await setup();
    mocks.executeMulti.mockResolvedValueOnce([{ ...emptyResult }]);
    await store.executeConsoleStatement(tabId, "SELECT 1 FROM DUAL;");
    mocks.executeMulti.mockRejectedValueOnce(new Error("ORA-00942: table or view does not exist"));
    await store.executeConsoleStatement(tabId, "SELECT missing FROM DUAL;");
    const session = store.consoleSessionFor(tabId);
    expect(session.entries.at(-1)).toMatchObject({ kind: "error", text: "ORA-00942: table or view does not exist" });
    expect(session.lastGoodSql).toBe("SELECT 1 FROM DUAL;");
    mocks.executeMulti.mockClear();
    mocks.executeMulti.mockResolvedValueOnce([{ ...emptyResult }]);
    await store.executeConsoleStatement(tabId, "/");
    expect(mocks.executeMulti.mock.calls[0][2]).toBe("SELECT 1 FROM DUAL;");
  });

  it("navigates history with up/down and restores the drafted input", async () => {
    const { store, tabId } = await setup();
    mocks.executeMulti.mockResolvedValue([{ ...emptyResult }]);
    await store.executeConsoleStatement(tabId, "SELECT 1;");
    await store.executeConsoleStatement(tabId, "SELECT 2;");
    expect(store.consoleHistoryNavigate(tabId, -1, "draft")).toEqual({ text: "SELECT 2;", fromHistory: true });
    expect(store.consoleHistoryNavigate(tabId, -1, "SELECT 2;")).toEqual({ text: "SELECT 1;", fromHistory: true });
    expect(store.consoleHistoryNavigate(tabId, 1, "SELECT 1;")).toEqual({ text: "SELECT 2;", fromHistory: true });
    expect(store.consoleHistoryNavigate(tabId, 1, "SELECT 2;")).toEqual({ text: "draft", fromHistory: false });
  });

  it("cancels the running statement through the execution id", async () => {
    mocks.cancelQuery.mockResolvedValue(true);
    const { store, tabId } = await setup();
    let release!: (results: QueryResult[]) => void;
    mocks.executeMulti.mockReturnValueOnce(
      new Promise<QueryResult[]>((resolve) => {
        release = resolve;
      }),
    );
    const run = store.executeConsoleStatement(tabId, "SELECT 1 FROM DUAL;");
    const session = store.consoleSessionFor(tabId);
    await vi.waitFor(() => expect(session.executionId).toBeDefined());
    await store.cancelConsoleStatement(tabId);
    expect(mocks.cancelQuery).toHaveBeenCalledWith(session.executionId);
    release([{ ...emptyResult }]);
    await run;
  });

  it.each(["/", "RUN"])("%s reruns only the last successful statement of a batch", async (rerun) => {
    const { store, tabId } = await setup();
    mocks.executeMulti.mockResolvedValueOnce([
      { ...emptyResult, statement_index: 0 },
      { ...emptyResult, statement_index: 1 },
    ]);
    await store.executeConsoleStatement(tabId, "INSERT INTO t VALUES (1);\nSELECT 1 FROM DUAL;");
    mocks.executeMulti.mockClear();
    await store.executeConsoleStatement(tabId, rerun);
    expect(mocks.executeMulti.mock.calls[0][2]).toBe("SELECT 1 FROM DUAL");
  });

  it("does not remember failed or skipped statements as the rerun target", async () => {
    const { store, tabId } = await setup();
    mocks.executeMulti.mockResolvedValueOnce([
      { ...emptyResult, statement_index: 0 },
      { ...emptyResult, statement_index: 1, execution_error: true },
    ]);
    await store.executeConsoleStatement(tabId, "SELECT 1 FROM DUAL;\nSELECT missing FROM DUAL;");
    expect(store.consoleSessionFor(tabId).lastGoodSql).toBe("SELECT 1 FROM DUAL");
    mocks.executeMulti.mockResolvedValueOnce([
      { ...emptyResult, statement_index: 0 },
      { ...emptyResult, statement_index: 1 },
    ]);
    await store.executeConsoleStatement(tabId, "SELECT 2 FROM DUAL;\nEXIT\nINSERT INTO t VALUES (1);");
    expect(store.consoleSessionFor(tabId).lastGoodSql).toBe("SELECT 2 FROM DUAL");
  });

  it("holds a PL/SQL block until its slash delimiter and executes it once", async () => {
    const { store, tabId } = await setup();
    for (const line of ["BEGIN", "  NULL;", "END;"]) await store.executeConsoleStatement(tabId, line);
    expect(mocks.executeMulti).not.toHaveBeenCalled();
    await store.executeConsoleStatement(tabId, "/");
    expect(mocks.executeMulti).toHaveBeenCalledOnce();
    expect(mocks.executeMulti.mock.calls[0][2]).toBe("BEGIN\n  NULL;\nEND;\n/");
  });

  it("preserves explicit MySQL transactions across submissions", async () => {
    mocks.getConfig.mockReturnValue({ id: "mysql-1", name: "MySQL", db_type: "mysql", database: "app", query_timeout_secs: 30 });
    const { store, tabId } = await setup("mysql-1", "app");
    for (const statement of ["START TRANSACTION;", "INSERT INTO t VALUES (1);", "ROLLBACK;"]) await store.executeConsoleStatement(tabId, statement);
    expect(mocks.executeMulti).toHaveBeenCalledTimes(3);
    for (const call of mocks.executeMulti.mock.calls) expect(call[5]).toMatchObject({ clientSessionId: tabId, preserveExplicitTransaction: true, timeoutSecs: 30 });
  });

  it("ignores an old target's response without clearing a newer execution", async () => {
    const { store, tabId } = await setup();
    let resolveOld!: (results: QueryResult[]) => void;
    let resolveNew!: (results: QueryResult[]) => void;
    mocks.executeMulti.mockReturnValueOnce(
      new Promise<QueryResult[]>((resolve) => {
        resolveOld = resolve;
      }),
    );
    const oldRun = store.executeConsoleStatement(tabId, "UPDATE t SET n = n + 1;");
    const oldSession = store.consoleSessionFor(tabId);
    const oldExecutionId = oldSession.executionId;
    store.updateDatabase(tabId, "OTHER");
    expect(mocks.cancelQuery).toHaveBeenCalledWith(oldExecutionId);
    mocks.executeMulti.mockReturnValueOnce(
      new Promise<QueryResult[]>((resolve) => {
        resolveNew = resolve;
      }),
    );
    const newRun = store.executeConsoleStatement(tabId, "SELECT 2 FROM DUAL;");
    const newSession = store.consoleSessionFor(tabId);
    expect(newSession).not.toBe(oldSession);
    resolveOld([{ ...emptyResult }]);
    await oldRun;
    expect(newSession.isExecuting).toBe(true);
    expect(newSession.lastGoodSql).toBeUndefined();
    expect(store.tabs.find((tab) => tab.id === tabId)?.executionId).toBe(newSession.executionId);
    resolveNew([{ ...emptyResult }]);
    await newRun;
    await store.executeConsoleStatement(tabId, "/");
    expect(mocks.executeMulti.mock.calls.at(-1)?.slice(0, 3)).toEqual(["oracle-1", "OTHER", "SELECT 2 FROM DUAL;"]);
  });

  it("does not let an old EXIT close a console after its target changes", async () => {
    const { store, tabId } = await setup();
    let resolveOld!: (results: QueryResult[]) => void;
    mocks.executeMulti.mockReturnValueOnce(
      new Promise<QueryResult[]>((resolve) => {
        resolveOld = resolve;
      }),
    );
    const run = store.executeConsoleStatement(tabId, "EXIT");
    store.updateDatabase(tabId, "OTHER");
    resolveOld([{ ...emptyResult }]);
    await run;
    expect(store.tabs.find((tab) => tab.id === tabId)?.database).toBe("OTHER");
  });

  it("exposes its busy state to generic cancellation and does not remember cancelled SQL", async () => {
    const { store, tabId } = await setup();
    await store.executeConsoleStatement(tabId, "SELECT 1 FROM DUAL;");
    let resolve!: (results: QueryResult[]) => void;
    mocks.executeMulti.mockReturnValueOnce(
      new Promise<QueryResult[]>((done) => {
        resolve = done;
      }),
    );
    const run = store.executeConsoleStatement(tabId, "UPDATE t SET n = n + 1;");
    const tab = store.tabs.find((tab) => tab.id === tabId)!;
    expect(tab.isExecuting).toBe(true);
    expect(tab.executionId).toBe(store.consoleSessionFor(tabId).executionId);
    expect(await store.cancelTabExecution(tabId)).toBe(true);
    resolve([{ ...emptyResult }]);
    await run;
    expect(tab.isExecuting).toBe(false);
    expect(tab.executionId).toBeUndefined();
    expect(store.consoleSessionFor(tabId).lastGoodSql).toBe("SELECT 1 FROM DUAL;");
  });

  it("cancels a running console query when its tab closes", async () => {
    const { store, tabId } = await setup();
    let resolve!: (results: QueryResult[]) => void;
    mocks.executeMulti.mockReturnValueOnce(
      new Promise<QueryResult[]>((done) => {
        resolve = done;
      }),
    );
    const run = store.executeConsoleStatement(tabId, "SELECT 1 FROM DUAL;");
    const executionId = store.consoleSessionFor(tabId).executionId;
    store.closeTab(tabId, { force: true });
    expect(mocks.cancelQuery).toHaveBeenCalledWith(executionId);
    resolve([{ ...emptyResult }]);
    await run;
    expect(store.tabs.find((tab) => tab.id === tabId)).toBeUndefined();
  });

  it("runs DM8 client commands on Enter and replays only the last statement", async () => {
    mocks.getConfig.mockReturnValue({ id: "dm-1", name: "DM8", db_type: "dameng", database: "", query_timeout_secs: 30 });
    const { store, tabId } = await setup("dm-1", "");
    for (const command of ["DESC t", "EXEC p(1)", "SHOW USER", "PROMPT dm8", "SET TIMING ON"]) {
      await store.executeConsoleStatement(tabId, command);
      expect(mocks.executeMulti.mock.calls.at(-1)?.[2]).toBe(command);
    }
    await store.executeConsoleStatement(tabId, "EXEC p(2)");
    await store.executeConsoleStatement(tabId, "RUN");
    expect(mocks.executeMulti.mock.calls.at(-1)?.[2]).toBe("EXEC p(2)");
    await store.executeConsoleStatement(tabId, "/");
    expect(mocks.executeMulti.mock.calls.at(-1)?.[2]).toBe("EXEC p(2)");
    mocks.executeMulti.mockClear();
    await store.executeConsoleStatement(tabId, "SET SCHEMA DBX_TEST");
    expect(mocks.executeMulti).not.toHaveBeenCalled();
    await store.executeConsoleStatement(tabId, ";");
    expect(mocks.executeMulti.mock.calls[0][2]).toBe("SET SCHEMA DBX_TEST\n;");
    await store.executeConsoleStatement(tabId, "EXIT");
    expect(store.tabs.find((tab) => tab.id === tabId)).toBeUndefined();
  });
});

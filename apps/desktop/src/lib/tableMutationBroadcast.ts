export interface TableMutationMessage {
  connectionId: string;
  database: string;
  schema?: string;
  tableName: string;
  sourceTabId?: string;
  sourceWindowId: string;
  nonce: string;
}

const CHANNEL_NAME = "dbx-table-mutations";
const STORAGE_KEY = "dbx-table-mutated";
const windowId = crypto.randomUUID();
const channel = typeof BroadcastChannel === "undefined" ? null : new BroadcastChannel(CHANNEL_NAME);

export function currentTableMutationWindowId() {
  return windowId;
}

export function broadcastTableMutation(message: Omit<TableMutationMessage, "sourceWindowId" | "nonce">) {
  const payload: TableMutationMessage = {
    ...message,
    sourceWindowId: windowId,
    nonce: crypto.randomUUID(),
  };
  channel?.postMessage(payload);
  localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
}

export function subscribeTableMutations(callback: (message: TableMutationMessage) => void) {
  const onMessage = (event: MessageEvent<TableMutationMessage>) => {
    if (event.data?.sourceWindowId !== windowId) callback(event.data);
  };
  const onStorage = (event: StorageEvent) => {
    if (event.key !== STORAGE_KEY || !event.newValue) return;
    try {
      const message = JSON.parse(event.newValue) as TableMutationMessage;
      if (message.sourceWindowId !== windowId) callback(message);
    } catch {}
  };

  channel?.addEventListener("message", onMessage);
  window.addEventListener("storage", onStorage);

  return () => {
    channel?.removeEventListener("message", onMessage);
    window.removeEventListener("storage", onStorage);
  };
}

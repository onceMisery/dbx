import { serializeOpenTabs } from "@/lib/openTabsPersistence";
import { isTauriRuntime } from "@/lib/tauriRuntime";
import type { QueryTab } from "@/types/database";
import { invoke } from "@tauri-apps/api/core";

const DETACHED_TAB_STORAGE_PREFIX = "dbx-detached-tab:";

function windowLabel() {
  return `main-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

function storeDetachedTab(tab: QueryTab): string {
  const key = windowLabel();
  localStorage.setItem(`${DETACHED_TAB_STORAGE_PREFIX}${key}`, JSON.stringify(serializeOpenTabs([tab])[0]));
  return key;
}

export function detachedTabStorageKey(key: string) {
  return `${DETACHED_TAB_STORAGE_PREFIX}${key}`;
}

export async function openDesktopWindow(options: { detachedTab?: QueryTab } = {}): Promise<boolean> {
  if (!isTauriRuntime()) return false;

  try {
    const detachedTabKey = options.detachedTab ? storeDetachedTab(options.detachedTab) : null;
    const url = detachedTabKey ? `/?detachedTab=${encodeURIComponent(detachedTabKey)}` : "/?newWindow=1";
    await invoke("open_new_desktop_window", { url });
    return true;
  } catch (error) {
    console.error("[DBX] Failed to open desktop window", error);
    return false;
  }
}

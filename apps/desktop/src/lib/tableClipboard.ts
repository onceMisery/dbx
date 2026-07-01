import type { DatabaseType } from "@/types/database";

export type PasteTableMode = "structure-and-data" | "structure-only" | "data-only";

export interface TableClipboardContext {
  connectionId: string;
  database: string;
  schema?: string | null;
}

const UNSAFE_WHOLE_ROW_COPY_DATABASES = new Set<DatabaseType>(["mysql", "postgres", "sqlserver"]);

function normalizeSchema(schema: string | null | undefined): string {
  return schema?.trim() ?? "";
}

export function tableClipboardEntryMatchesTarget(entry: TableClipboardContext, target: TableClipboardContext): boolean {
  return entry.connectionId === target.connectionId && entry.database === target.database && normalizeSchema(entry.schema) === normalizeSchema(target.schema);
}

export function tableClipboardMatchesTarget(entries: TableClipboardContext[], target: TableClipboardContext | null): boolean {
  return !!target && entries.length > 0 && entries.every((entry) => tableClipboardEntryMatchesTarget(entry, target));
}

export function supportsWholeRowTableDataCopy(databaseType: DatabaseType | undefined): boolean {
  return !!databaseType && !UNSAFE_WHOLE_ROW_COPY_DATABASES.has(databaseType);
}

export function defaultPasteTableMode(databaseType: DatabaseType | undefined): PasteTableMode {
  return supportsWholeRowTableDataCopy(databaseType) ? "structure-and-data" : "structure-only";
}

export function pasteTableModeCopiesData(mode: PasteTableMode): boolean {
  return mode === "structure-and-data" || mode === "data-only";
}

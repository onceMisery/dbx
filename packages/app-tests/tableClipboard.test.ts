import { strict as assert } from "node:assert";
import { test } from "vitest";
import { defaultPasteTableMode, pasteTableModeCopiesData, supportsWholeRowTableDataCopy, tableClipboardMatchesTarget } from "../../apps/desktop/src/lib/tableClipboard.ts";

test("table clipboard entries must match the paste target context", () => {
  const target = { connectionId: "c1", database: "app", schema: "public" };

  assert.equal(tableClipboardMatchesTarget([{ connectionId: "c1", database: "app", schema: "public" }], target), true);
  assert.equal(tableClipboardMatchesTarget([{ connectionId: "c2", database: "app", schema: "public" }], target), false);
  assert.equal(tableClipboardMatchesTarget([{ connectionId: "c1", database: "other", schema: "public" }], target), false);
  assert.equal(tableClipboardMatchesTarget([{ connectionId: "c1", database: "app", schema: "audit" }], target), false);
  assert.equal(tableClipboardMatchesTarget([], target), false);
  assert.equal(tableClipboardMatchesTarget([{ connectionId: "c1", database: "app" }], null), false);
});

test("whole-row table data copy is disabled for generated-column-prone databases", () => {
  assert.equal(supportsWholeRowTableDataCopy("mysql"), false);
  assert.equal(supportsWholeRowTableDataCopy("postgres"), false);
  assert.equal(supportsWholeRowTableDataCopy("sqlserver"), false);
  assert.equal(supportsWholeRowTableDataCopy(undefined), false);
  assert.equal(supportsWholeRowTableDataCopy("sqlite"), true);
  assert.equal(defaultPasteTableMode("mysql"), "structure-only");
  assert.equal(defaultPasteTableMode(undefined), "structure-only");
  assert.equal(defaultPasteTableMode("sqlite"), "structure-and-data");
  assert.equal(pasteTableModeCopiesData("structure-and-data"), true);
  assert.equal(pasteTableModeCopiesData("data-only"), true);
  assert.equal(pasteTableModeCopiesData("structure-only"), false);
});

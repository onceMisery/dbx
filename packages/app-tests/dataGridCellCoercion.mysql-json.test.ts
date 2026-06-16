import { strict as assert } from "node:assert";
import { test } from "vitest";
import { coerceDataGridCellValue } from "../../apps/desktop/src/lib/dataGridCellCoercion.ts";

test("MySQL JSON field with English quotes should remain unchanged", () => {
  const input = '{"2:3":"3:4","3:2":"4:3","21:9":"16:9"}';
  const result = coerceDataGridCellValue({
    value: input,
    oldValue: null,
    databaseType: "mysql",
    columnInfo: { data_type: "json" },
  });

  assert.equal(result, input);
  assert.ok(result.includes('"'), "Should contain English quotes");
  assert.ok(!result.includes('“') && !result.includes('”'), "Should NOT contain Chinese quotes");
});

test("MySQL JSON field with Chinese quotes should be normalized to English quotes", () => {
  const input = '{"2:3":"3:4","3:2":"4:3","21:9":"16:9"}'; // 中文引号
  const expected = '{"2:3":"3:4","3:2":"4:3","21:9":"16:9"}'; // 英文引号

  const result = coerceDataGridCellValue({
    value: input,
    oldValue: null,
    databaseType: "mysql",
    columnInfo: { data_type: "json" },
  });

  assert.equal(result, expected);
  assert.ok(result.includes('"'), "Should contain English quotes");
  assert.ok(!result.includes('“') && !result.includes('”'), "Should NOT contain Chinese quotes");
});

test("MySQL JSON field with mixed quotes should be normalized", () => {
  const input = '{"key":"value"}'; // 混合了中英文引号
  const expected = '{"key":"value"}'; // 全部英文引号

  const result = coerceDataGridCellValue({
    value: input,
    oldValue: null,
    databaseType: "mysql",
    columnInfo: { data_type: "json" },
  });

  assert.equal(result, expected);
});

import { describe, expect, test } from "vitest";
import { backendResponseError } from "@/lib/backend/http";

const envelope = {
  version: 1,
  code: "DBX-JDBC-4001",
  messageKey: "backendErrors.jdbc.sqlFailed",
  messageParams: { stage: "execute" },
  source: "jdbcAgent",
  operationOutcome: "unknown",
  detail: "Incorrect syntax near SELECT",
} as const;

describe("HTTP backend error parsing", () => {
  test.each([
    ["direct envelope", JSON.stringify(envelope), envelope],
    ["nested envelope", JSON.stringify({ error: envelope }), envelope],
    ["legacy text", "relation missing_table does not exist", undefined],
    ["malformed JSON text", "{not-json", undefined],
  ])("preserves %s body diagnostics", async (_name, body, expected) => {
    const error = await backendResponseError(new Response(body, { status: 500 }));
    if (expected) {
      expect(error.backendError).toEqual(expected);
    } else {
      expect(error.backendError.code).toBe("DBX-LEGACY-0001");
      expect(error.backendError.detail).toBe(body);
    }
  });

  test("uses a stable summary for an empty body", async () => {
    const error = await backendResponseError(new Response("", { status: 503 }));
    expect(error.backendError.code).toBe("DBX-LEGACY-0001");
    expect(error.backendError.detail).toBeUndefined();
    expect(error.message).toBe("Backend request failed");
  });

  test("keeps a safe SQL diagnostic in a JSON envelope unchanged", async () => {
    const error = await backendResponseError(new Response(JSON.stringify(envelope), { status: 400 }));
    expect(error.backendError.detail).toBe("Incorrect syntax near SELECT");
  });
});

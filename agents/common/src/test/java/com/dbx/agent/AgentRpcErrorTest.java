package com.dbx.agent;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.sql.SQLException;
import java.sql.SQLTimeoutException;
import java.util.concurrent.CancellationException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;

class AgentRpcErrorTest {
    @Test
    void serializesVersionedSqlDiagnosticsAndUnknownExecuteOutcome() {
        SQLException cause = new SQLException("statement failed", "42000", 1064);

        JsonObject data = errorData(cause, AgentProtocol.METHOD_EXECUTE_QUERY, "session-1");

        assertEquals(1, data.get("contractVersion").getAsInt());
        assertEquals("sql", data.get("category").getAsString());
        assertEquals("execute", data.get("stage").getAsString());
        assertEquals("unknown", data.get("operationOutcome").getAsString());
        assertEquals("keep", data.get("sessionDisposition").getAsString());
        assertEquals("42000", data.get("sqlState").getAsString());
        assertEquals(1064, data.get("vendorCode").getAsInt());
        assertEquals(SQLException.class.getName(), data.get("exceptionClass").getAsString());
        assertEquals("session-1", data.get("agentSessionId").getAsString());
    }

    @Test
    void classifiesTimeoutAndCanceledErrorsWithoutClaimingSafeReplay() {
        JsonObject timeout = errorData(
            new SQLTimeoutException("timed out", "HYT00", 0),
            AgentProtocol.METHOD_EXECUTE_QUERY,
            "session-1"
        );
        JsonObject canceled = errorData(
            new CancellationException("canceled"),
            AgentProtocol.METHOD_CANCEL_SESSION,
            "session-1"
        );

        assertEquals("timeout", timeout.get("category").getAsString());
        assertEquals("unknown", timeout.get("operationOutcome").getAsString());
        assertFalse(timeout.get("retryable").getAsBoolean());
        assertEquals("canceled", canceled.get("category").getAsString());
        assertEquals("cancel", canceled.get("stage").getAsString());
        assertEquals("unknown", canceled.get("operationOutcome").getAsString());
    }

    @Test
    void marksConnectionSetupFailureAsNotStartedAndBoundsSqlState() {
        SQLException cause = new SQLException("connect failed", "12345678901234567890", -7);

        JsonObject data = errorData(cause, AgentProtocol.METHOD_CONNECT, null);

        assertEquals("connection", data.get("category").getAsString());
        assertEquals("connect", data.get("stage").getAsString());
        assertEquals("not_started", data.get("operationOutcome").getAsString());
        assertEquals(16, data.get("sqlState").getAsString().length());
        assertFalse(data.has("agentSessionId"));
    }

    private static JsonObject errorData(Throwable error, String method, String agentSessionId) {
        return AgentRpcError.toJson(error, method, agentSessionId).getAsJsonObject("data");
    }
}

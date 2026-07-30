package com.dbx.agent;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.dbx.agent.dameng.DamengAgent;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.time.Duration;
import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.ResultSet;
import java.sql.Statement;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;

class DamengLiveTimeoutRecoveryTest {
    private static final Gson GSON = new Gson();

    @Test
    void rawJdbcCursorSupportsLookahead() throws Exception {
        LiveConfig config = LiveConfig.fromEnvironment();
        Assumptions.assumeTrue(config != null, "DM8 live test environment is not configured");
        Class.forName("dm.jdbc.driver.DmDriver");

        try (Connection connection = DriverManager.getConnection(config.jdbcUrl(), config.username, config.password);
             Statement statement = connection.createStatement();
             ResultSet resultSet = executeWithTimeout(statement, "SELECT LEVEL AS ID FROM DUAL CONNECT BY LEVEL <= 3")) {
            assertEquals("ID", resultSet.getMetaData().getColumnLabel(1));
            resultSet.getMetaData().getColumnType(1);
            resultSet.getMetaData().getColumnTypeName(1);
            assertTrue(resultSet.next());
            assertEquals(1, resultSet.getInt(1));
            assertTrue(resultSet.next());
            assertEquals(2, resultSet.getInt(1));
        }
    }

    private static ResultSet executeWithTimeout(Statement statement, String sql) throws Exception {
        statement.setQueryTimeout(2);
        assertTrue(statement.execute(sql));
        return statement.getResultSet();
    }

    @Test
    void pagedQueryKeepsItsCursorOpenAcrossRequests() {
        LiveConfig config = LiveConfig.fromEnvironment();
        Assumptions.assumeTrue(config != null, "DM8 live test environment is not configured");
        MultiSessionJsonRpcServer server = new MultiSessionJsonRpcServer(DamengAgent::new, true);

        assertResult(server, request(1, "open_session", config.openParams("paging-live", 1L, "WORKLOAD")));
        Map<String, Object> pageParams = operationParams(
            "paging-live",
            1L,
            "page-start",
            "WORKLOAD",
            "SELECT LEVEL AS ID FROM DUAL CONNECT BY LEVEL <= 3"
        );
        pageParams.put("pageSize", 1);
        JsonObject firstPage = assertResult(server, request(2, "execute_query_page", pageParams));
        assertTrue(firstPage.get("has_more").getAsBoolean());

        Map<String, Object> fetchParams = controlParams("paging-live", 1L, "page-fetch");
        fetchParams.put("lane", "WORKLOAD");
        fetchParams.put("sessionId", firstPage.get("session_id").getAsString());
        fetchParams.put("pageSize", 1);
        JsonObject secondPage = assertResult(server, request(3, "fetch_query_page", fetchParams));
        assertEquals(1, secondPage.getAsJsonArray("rows").size());
    }

    @Test
    void blockedWorkloadDoesNotBlockMetadataAndCanBeReplaced() {
        LiveConfig config = LiveConfig.fromEnvironment();
        Assumptions.assumeTrue(config != null, "DM8 live test environment is not configured");
        MultiSessionJsonRpcServer server = new MultiSessionJsonRpcServer(DamengAgent::new, true);

        assertResult(server, request(1, "open_session", config.openParams("metadata-live", 1L, "METADATA")));
        assertResult(server, request(2, "open_session", config.openParams("workload-live", 1L, "WORKLOAD")));
        Map<String, Object> oldPageParams = operationParams(
            "workload-live",
            1L,
            "old-page-start",
            "WORKLOAD",
            "SELECT LEVEL AS ID FROM DUAL CONNECT BY LEVEL <= 3"
        );
        oldPageParams.put("pageSize", 1);
        String oldCursor = assertResult(server, request(21, "execute_query_page", oldPageParams))
            .get("session_id")
            .getAsString();

        Thread blocked = new Thread(() -> server.handleRequest(request(
            3,
            "execute_query",
            operationParams("workload-live", 1L, "blocked-call", "WORKLOAD", "CALL SP_SLEEP(30)")
        )), "dameng-live-blocked-query");
        blocked.setDaemon(true);
        blocked.start();
        sleep(Duration.ofSeconds(1));

        long metadataStarted = System.nanoTime();
        assertResult(server, request(
            4,
            "execute_query",
            operationParams("metadata-live", 1L, "metadata-probe", "METADATA", "SELECT 1")
        ));
        assertTrue(elapsedMillis(metadataStarted) < 5_000L, "metadata lane must remain responsive");

        JsonObject cancel = assertResult(server, request(
            5,
            "cancel_operation",
            controlParams("workload-live", 1L, "blocked-call")
        ));
        assertTrue(cancel.get("accepted").getAsBoolean());
        JsonObject quarantine = assertResult(server, request(
            6,
            "quarantine_session",
            controlParams("workload-live", 1L, "blocked-call")
        ));
        assertEquals("QUARANTINED", quarantine.get("state").getAsString());
        Map<String, Object> staleFetch = controlParams("workload-live", 1L, "stale-page-fetch");
        staleFetch.put("lane", "WORKLOAD");
        staleFetch.put("sessionId", oldCursor);
        staleFetch.put("pageSize", 1);
        assertError(server, request(22, "fetch_query_page", staleFetch));

        assertResult(server, request(7, "open_session", config.openParams("workload-live-2", 2L, "WORKLOAD")));
        assertResult(server, request(
            8,
            "execute_query",
            operationParams("workload-live-2", 2L, "replacement-probe", "WORKLOAD", "SELECT 1")
        ));
        Map<String, Object> pageParams = operationParams(
            "workload-live-2",
            2L,
            "page-start",
            "WORKLOAD",
            "SELECT LEVEL AS ID FROM DUAL CONNECT BY LEVEL <= 3"
        );
        pageParams.put("pageSize", 1);
        JsonObject firstPage = assertResult(server, request(9, "execute_query_page", pageParams));
        assertTrue(firstPage.get("has_more").getAsBoolean());
        Map<String, Object> fetchParams = controlParams("workload-live-2", 2L, "page-fetch");
        fetchParams.put("lane", "WORKLOAD");
        fetchParams.put("sessionId", firstPage.get("session_id").getAsString());
        fetchParams.put("pageSize", 1);
        JsonObject secondPage = assertResult(server, request(10, "fetch_query_page", fetchParams));
        assertEquals(1, secondPage.getAsJsonArray("rows").size());
    }

    @Test
    void timedOutTransactionScopeIsQuarantinedBeforeReplacement() {
        LiveConfig config = LiveConfig.fromEnvironment();
        Assumptions.assumeTrue(config != null, "DM8 live test environment is not configured");
        MultiSessionJsonRpcServer server = new MultiSessionJsonRpcServer(DamengAgent::new, true);

        assertResult(server, request(31, "open_session", config.openParams("transaction-live", 1L, "WORKLOAD")));
        Map<String, Object> transactionParams = controlParams("transaction-live", 1L, "blocked-transaction");
        transactionParams.put("lane", "WORKLOAD");
        transactionParams.put("statements", java.util.List.of("CALL SP_SLEEP(30)"));
        Thread blocked = new Thread(
            () -> server.handleRequest(request(32, "execute_transaction", transactionParams)),
            "dameng-live-blocked-transaction"
        );
        blocked.setDaemon(true);
        blocked.start();
        sleep(Duration.ofSeconds(1));

        JsonObject cancel = assertResult(server, request(
            33,
            "cancel_operation",
            controlParams("transaction-live", 1L, "blocked-transaction")
        ));
        assertTrue(cancel.get("accepted").getAsBoolean());
        assertResult(server, request(
            34,
            "quarantine_session",
            controlParams("transaction-live", 1L, "blocked-transaction")
        ));
        assertResult(server, request(35, "open_session", config.openParams("transaction-live-2", 2L, "WORKLOAD")));
        assertResult(server, request(
            36,
            "execute_query",
            operationParams("transaction-live-2", 2L, "transaction-replacement-probe", "WORKLOAD", "SELECT 1")
        ));
    }

    private static Map<String, Object> operationParams(
        String sessionId,
        long generation,
        String operationId,
        String lane,
        String sql
    ) {
        Map<String, Object> params = controlParams(sessionId, generation, operationId);
        params.put("lane", lane);
        params.put("sql", sql);
        params.put("maxRows", 10);
        params.put("timeoutSecs", 2);
        return params;
    }

    private static Map<String, Object> controlParams(String sessionId, long generation, String operationId) {
        Map<String, Object> params = new LinkedHashMap<>();
        params.put("agentSessionId", sessionId);
        params.put("generation", generation);
        params.put("operationId", operationId);
        return params;
    }

    private static String request(long id, String method, Map<String, Object> params) {
        Map<String, Object> request = new LinkedHashMap<>();
        request.put("jsonrpc", "2.0");
        request.put("id", id);
        request.put("method", method);
        request.put("params", params);
        return GSON.toJson(request);
    }

    private static JsonObject assertResult(MultiSessionJsonRpcServer server, String request) {
        JsonObject response = JsonParser.parseString(server.handleRequest(request)).getAsJsonObject();
        assertTrue(response.has("result"), () -> response.has("error")
            ? response.getAsJsonObject("error").get("message").getAsString()
            : "Agent response has no result");
        return response.getAsJsonObject("result");
    }

    private static JsonObject assertError(MultiSessionJsonRpcServer server, String request) {
        JsonObject response = JsonParser.parseString(server.handleRequest(request)).getAsJsonObject();
        assertTrue(response.has("error"), "Agent response must reject stale session state");
        return response.getAsJsonObject("error");
    }

    private static long elapsedMillis(long started) {
        return TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);
    }

    private static void sleep(Duration duration) {
        try {
            Thread.sleep(duration.toMillis());
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            throw new IllegalStateException("Interrupted while preparing live DM8 assertion", error);
        }
    }

    private static final class LiveConfig {
        private final String host;
        private final int port;
        private final String username;
        private final String password;

        private LiveConfig(String host, int port, String username, String password) {
            this.host = host;
            this.port = port;
            this.username = username;
            this.password = password;
        }

        private Map<String, Object> openParams(String sessionId, long generation, String lane) {
            Map<String, Object> params = operationParams(sessionId, generation, "open-" + sessionId, lane, "");
            params.remove("sql");
            params.remove("maxRows");
            params.remove("timeoutSecs");
            params.put("host", host);
            params.put("port", port);
            params.put("database", "");
            params.put("username", username);
            params.put("password", password);
            return params;
        }

        private String jdbcUrl() {
            return "jdbc:dm://" + host + ":" + port;
        }

        private static LiveConfig fromEnvironment() {
            String endpoint = System.getenv("DBX_DM_TEST_URL");
            String username = System.getenv("DBX_DM_TEST_USER");
            String password = System.getenv("DBX_DM_TEST_PASSWORD");
            if (isBlank(endpoint) || isBlank(username) || isBlank(password)) return null;
            String normalized = endpoint.trim().replaceFirst("^jdbc:dm://", "");
            int separator = normalized.lastIndexOf(':');
            if (separator <= 0 || separator == normalized.length() - 1) {
                throw new IllegalArgumentException("DBX_DM_TEST_URL must be host:port or jdbc:dm://host:port");
            }
            return new LiveConfig(
                normalized.substring(0, separator),
                Integer.parseInt(normalized.substring(separator + 1)),
                username,
                password
            );
        }

        private static boolean isBlank(String value) {
            return value == null || value.trim().isEmpty();
        }
    }
}

package com.dbx.agent;

import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.sql.SQLException;
import java.sql.SQLTimeoutException;
import java.util.Collections;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.RejectedExecutionException;
import java.util.function.Supplier;
import com.dbx.agent.runtime.OperationContext;
import com.dbx.agent.runtime.OperationRegistry;
import com.dbx.agent.runtime.OperationState;
import com.dbx.agent.runtime.SessionRuntime;
import com.dbx.agent.runtime.Lane;

public final class MultiSessionJsonRpcServer {
    private static final String LEGACY_SESSION_ID = "__legacy__";
    private static final int MAX_SESSIONS = 256;

    private final Supplier<? extends DatabaseAgent> agentFactory;
    private final boolean recoverableSessions;
    private final Map<String, Session> sessions = new ConcurrentHashMap<>();
    private final ExecutorService dataRequests = boundedExecutor(4, 256, "agent-data");
    private final ExecutorService controlRequests = boundedExecutor(2, 64, "agent-control");
    private final ExecutorService cleanupRequests = boundedExecutor(1, 64, "agent-cleanup");
    private final ExecutorService cleanupOverflowRequests = boundedExecutor(1, MAX_SESSIONS, "agent-cleanup-overflow");
    private final Gson gson = new Gson();
    private final Object outputLock = AgentOutput.LOCK;

    public MultiSessionJsonRpcServer(Supplier<? extends DatabaseAgent> agentFactory) {
        this(agentFactory, false);
    }

    public MultiSessionJsonRpcServer(Supplier<? extends DatabaseAgent> agentFactory, boolean recoverableSessions) {
        this.agentFactory = agentFactory;
        this.recoverableSessions = recoverableSessions;
    }

    public void run() {
        synchronized (outputLock) {
            System.out.println("{\"ready\":true}");
            System.out.flush();
        }
        try (BufferedReader reader = new BufferedReader(new InputStreamReader(System.in))) {
            String line;
            while ((line = reader.readLine()) != null) {
                JsonObject request = JsonParser.parseString(line).getAsJsonObject();
                String method = request.get("method").getAsString();
                if (AgentProtocol.METHOD_SHUTDOWN.equals(method)) {
                    writeResponse(handleRequest(request));
                    closeAllSessions();
                    dataRequests.shutdown();
                    controlRequests.shutdown();
                    cleanupRequests.shutdown();
                    cleanupOverflowRequests.shutdown();
                    return;
                }
                ExecutorService executor = isControlMethod(method) ? controlRequests : dataRequests;
                try {
                    executor.submit(() -> writeResponse(handleRequest(request)));
                } catch (RejectedExecutionException overloaded) {
                    writeResponse(errorResponse(request, "AGENT_OVERLOADED", true, "REUSABLE", "Agent request queue is full"));
                }
            }
        } catch (Exception e) {
            closeAllSessions();
            throw new RuntimeException(e);
        }
    }

    String handleRequest(String line) {
        return gson.toJson(handleRequest(JsonParser.parseString(line).getAsJsonObject()));
    }

    private JsonObject handleRequest(JsonObject request) {
        JsonElement id = request.get("id");
        String method = request.get("method").getAsString();
        JsonObject params = request.has("params") && request.get("params").isJsonObject()
            ? request.getAsJsonObject("params")
            : new JsonObject();
        JsonObject response = new JsonObject();
        response.addProperty("jsonrpc", "2.0");
        response.add("id", id);
        try {
            Object result;
            if (AgentProtocol.METHOD_HANDSHAKE.equals(method)) {
                result = recoverableSessions
                    ? AgentProtocol.recoverableSessionHandshakeResult()
                    : AgentProtocol.multiSessionHandshakeResult();
            } else if (AgentProtocol.METHOD_OPEN_SESSION.equals(method)) {
                result = openSession(requiredSessionId(params), params);
            } else if (AgentProtocol.METHOD_CLOSE_SESSION.equals(method)) {
                String sessionId = requiredSessionId(params);
                if (recoverableSessions) session(sessionId).assertGeneration(requiredGeneration(params));
                result = closeSession(sessionId);
            } else if (AgentProtocol.METHOD_VALIDATE_SESSION.equals(method)) {
                result = session(requiredSessionId(params)).handle("validate_connection", params);
            } else if (AgentProtocol.METHOD_CANCEL_SESSION.equals(method)) {
                session(requiredSessionId(params)).cancel();
                result = Collections.singletonMap("ok", true);
            } else if (AgentProtocol.METHOD_CANCEL_OPERATION.equals(method)) {
                Session session = session(requiredSessionId(params));
                result = session.cancelOperation(requiredOperationId(params), requiredGeneration(params));
            } else if (AgentProtocol.METHOD_QUARANTINE_SESSION.equals(method)) {
                Session session = session(requiredSessionId(params));
                session.quarantine(requiredGeneration(params));
                result = session.status();
            } else if (AgentProtocol.METHOD_SESSION_STATUS.equals(method)) {
                Session session = session(requiredSessionId(params));
                session.assertGeneration(requiredGeneration(params));
                result = session.status();
            } else if (AgentProtocol.METHOD_TEST_CONNECTION.equals(method)) {
                result = new JsonRpcServer(agentFactory.get()).dispatchForRuntime(method, params);
            } else if (AgentProtocol.METHOD_CONNECT.equals(method)) {
                closeSession(LEGACY_SESSION_ID);
                result = openSession(LEGACY_SESSION_ID, params);
            } else if (AgentProtocol.METHOD_DISCONNECT.equals(method)) {
                result = closeSession(LEGACY_SESSION_ID);
            } else if (AgentProtocol.METHOD_SHUTDOWN.equals(method)) {
                result = Collections.singletonMap("ok", true);
            } else {
                String sessionId = params.has("agentSessionId") ? params.get("agentSessionId").getAsString() : LEGACY_SESSION_ID;
                result = session(sessionId).handle(method, params);
            }
            response.add("result", gson.toJsonTree(result));
        } catch (Throwable error) {
            JsonObject rpcError = new JsonObject();
            rpcError.addProperty("code", -1);
            rpcError.addProperty("message", error.getMessage() == null ? error.toString() : error.getMessage());
            if (recoverableSessions) {
                String message = rpcError.get("message").getAsString();
                String category = message.contains("Stale Agent session generation")
                    ? "STALE_GENERATION"
                    : message.contains("quarantined")
                        ? "SESSION_QUARANTINED"
                        : isConnectionFailure(error)
                            ? "CONNECTION_BROKEN"
                            : isTimeout(error) ? "QUERY_TIMEOUT" : "DATABASE_ERROR";
                String disposition = "DATABASE_ERROR".equals(category) ? "REUSABLE" : "QUARANTINE_SESSION";
                JsonObject data = errorData(params, category, "QUERY_TIMEOUT".equals(category), disposition);
                SQLException sqlError = findSqlException(error);
                if (sqlError != null) data.addProperty("driverCode", Integer.toString(sqlError.getErrorCode()));
                rpcError.add("data", data);
            }
            response.add("error", rpcError);
        }
        return response;
    }

    private Object openSession(String sessionId, JsonObject params) throws Exception {
        if (sessions.size() >= MAX_SESSIONS && !sessions.containsKey(sessionId)) {
            throw new IllegalStateException("Agent session limit reached: " + MAX_SESSIONS);
        }
        if (recoverableSessions) {
            requiredGeneration(params);
            requiredOperationId(params);
            requiredLane(params);
        }
        long generation = params.has("generation") ? params.get("generation").getAsLong() : 0L;
        Lane lane = recoverableSessions ? Lane.parse(params.get("lane").getAsString()) : Lane.WORKLOAD;
        Session session = new Session(sessionId, generation, lane, new JsonRpcServer(agentFactory.get()));
        Session existing = sessions.putIfAbsent(sessionId, session);
        if (existing != null) {
            throw new IllegalStateException("Agent session already exists: " + sessionId);
        }
        try {
            return session.handle(AgentProtocol.METHOD_CONNECT, params);
        } catch (Exception error) {
            sessions.remove(sessionId, session);
            session.close();
            throw error;
        }
    }

    private Object closeSession(String sessionId) {
        Session session = sessions.remove(sessionId);
        if (session != null) {
            if (recoverableSessions) {
                session.quarantine(session.generation());
                submitCleanup(session::close);
            } else {
                session.close();
            }
        }
        return Collections.singletonMap("ok", true);
    }

    private Session session(String sessionId) {
        Session session = sessions.get(sessionId);
        if (session == null) {
            throw new IllegalStateException("Agent session not found: " + sessionId);
        }
        return session;
    }

    private void closeAllSessions() {
        for (String sessionId : sessions.keySet()) {
            closeSession(sessionId);
        }
    }

    private static String requiredSessionId(JsonObject params) {
        if (!params.has("agentSessionId") || params.get("agentSessionId").getAsString().trim().isEmpty()) {
            throw new IllegalArgumentException("agentSessionId is required");
        }
        return params.get("agentSessionId").getAsString();
    }

    private static String requiredOperationId(JsonObject params) {
        if (!params.has("operationId") || params.get("operationId").getAsString().trim().isEmpty()) {
            throw new IllegalArgumentException("operationId is required");
        }
        return params.get("operationId").getAsString();
    }

    private static long requiredGeneration(JsonObject params) {
        if (!params.has("generation")) throw new IllegalArgumentException("generation is required");
        return params.get("generation").getAsLong();
    }

    private static String requiredLane(JsonObject params) {
        if (!params.has("lane") || params.get("lane").getAsString().trim().isEmpty()) {
            throw new IllegalArgumentException("lane is required");
        }
        String lane = params.get("lane").getAsString();
        Lane.parse(lane);
        return lane;
    }

    private static boolean isControlMethod(String method) {
        return AgentProtocol.METHOD_CANCEL_SESSION.equals(method)
            || AgentProtocol.METHOD_CANCEL_OPERATION.equals(method)
            || AgentProtocol.METHOD_QUARANTINE_SESSION.equals(method)
            || AgentProtocol.METHOD_SESSION_STATUS.equals(method)
            || AgentProtocol.METHOD_CLOSE_SESSION.equals(method)
            || AgentProtocol.METHOD_SHUTDOWN.equals(method);
    }

    private static ExecutorService boundedExecutor(int threads, int queueCapacity, String prefix) {
        java.util.concurrent.atomic.AtomicLong sequence = new java.util.concurrent.atomic.AtomicLong();
        return new ThreadPoolExecutor(
            threads, threads, 0L, TimeUnit.MILLISECONDS, new ArrayBlockingQueue<>(queueCapacity),
            runnable -> {
                Thread thread = new Thread(runnable, prefix + "-" + sequence.incrementAndGet());
                thread.setDaemon(true);
                return thread;
            },
            new ThreadPoolExecutor.AbortPolicy()
        );
    }

    private void submitCleanup(Runnable cleanup) {
        try {
            cleanupRequests.submit(cleanup);
        } catch (RejectedExecutionException saturated) {
            cleanupOverflowRequests.submit(cleanup);
        }
    }

    private JsonObject errorResponse(
        JsonObject request,
        String category,
        boolean retryable,
        String disposition,
        String message
    ) {
        JsonObject response = new JsonObject();
        response.addProperty("jsonrpc", "2.0");
        response.add("id", request.get("id"));
        JsonObject error = new JsonObject();
        error.addProperty("code", -32001);
        error.addProperty("message", message);
        JsonObject params = request.has("params") && request.get("params").isJsonObject()
            ? request.getAsJsonObject("params")
            : new JsonObject();
        error.add("data", errorData(params, category, retryable, disposition));
        response.add("error", error);
        return response;
    }

    private static JsonObject errorData(
        JsonObject params,
        String category,
        boolean retryable,
        String disposition
    ) {
        JsonObject data = new JsonObject();
        data.addProperty("category", category);
        data.addProperty("retryable", retryable);
        data.addProperty("connectionDisposition", disposition);
        data.addProperty("operationId", params.has("operationId") ? params.get("operationId").getAsString() : "");
        data.addProperty("agentSessionId", params.has("agentSessionId") ? params.get("agentSessionId").getAsString() : "");
        data.addProperty("generation", params.has("generation") ? params.get("generation").getAsLong() : 0L);
        return data;
    }

    private static boolean isConnectionFailure(Throwable error) {
        for (Throwable current = error; current != null; current = current.getCause()) {
            if (current instanceof SQLException) {
                String sqlState = ((SQLException) current).getSQLState();
                if (sqlState != null && sqlState.startsWith("08")) return true;
            }
        }
        return false;
    }

    private static boolean isTimeout(Throwable error) {
        for (Throwable current = error; current != null; current = current.getCause()) {
            if (current instanceof SQLTimeoutException) return true;
            if (current instanceof SQLException && ((SQLException) current).getErrorCode() == -608) return true;
            String message = current.getMessage();
            if (message != null && message.toLowerCase(java.util.Locale.ROOT).contains("timeout")) return true;
        }
        return false;
    }

    private static SQLException findSqlException(Throwable error) {
        for (Throwable current = error; current != null; current = current.getCause()) {
            if (current instanceof SQLException) return (SQLException) current;
        }
        return null;
    }

    private void writeResponse(JsonObject response) {
        synchronized (outputLock) {
            System.out.println(gson.toJson(response));
            System.out.flush();
        }
    }

    private static final class Session {
        private final String sessionId;
        private final JsonRpcServer server;
        private final OperationRegistry operations = new OperationRegistry();
        private final SessionRuntime runtime;
        private final Lane lane;
        private final Object metadataLock = new Object();
        private final Object workloadLock = new Object();

        private Session(String sessionId, long generation, Lane lane, JsonRpcServer server) {
            this.sessionId = sessionId;
            this.lane = lane;
            this.server = server;
            this.runtime = new SessionRuntime(generation, operations);
        }

        private Object handle(String method, JsonObject params) throws Exception {
            if (runtime.generation() > 0L) {
                requiredGeneration(params);
                requiredOperationId(params);
                requiredLane(params);
            }
            long generation = params.has("generation") ? params.get("generation").getAsLong() : runtime.generation();
            runtime.assertDataRequestAllowed(generation);
            OperationContext context = OperationContext.from(params, sessionId, generation);
            if (runtime.generation() > 0L && context.lane() != lane) {
                throw new IllegalStateException("Agent Session lane mismatch");
            }
            operations.start(context);
            try {
                Object result;
                synchronized (laneLock(context.lane())) {
                    runtime.assertDataRequestAllowed(generation);
                    server.beginOperation(method);
                    try {
                        result = server.dispatchForRuntime(method, params, context, operations);
                    } finally {
                        server.endOperation();
                    }
                }
                operations.finish(context.operationId(), OperationState.SUCCEEDED);
                return result;
            } catch (Exception error) {
                OperationState current = operations.state(context.operationId());
                operations.finish(
                    context.operationId(),
                    current == OperationState.CANCEL_REQUESTED ? OperationState.CANCELLED_CONFIRMED : OperationState.FAILED
                );
                throw error;
            } finally {
                runtime.operationCompleted();
            }
        }

        private Object laneLock(Lane lane) {
            return lane == Lane.METADATA ? metadataLock : workloadLock;
        }

        private void close() {
            runtime.retiring();
            try {
                server.dispatchForRuntime(AgentProtocol.METHOD_DISCONNECT, new JsonObject());
            } catch (Exception ignored) {
            } finally {
                runtime.retired();
            }
        }

        private void cancel() {
            server.cancelActiveStatements();
        }

        private Object cancelOperation(String operationId, long generation) {
            assertGeneration(generation);
            java.util.Map<String, Object> result = new java.util.LinkedHashMap<>();
            result.put("accepted", runtime.cancel(operationId));
            OperationState state = operations.state(operationId);
            result.put("state", state == null ? "NOT_FOUND" : state.name());
            return result;
        }

        private void quarantine(long generation) {
            assertGeneration(generation);
            runtime.quarantine();
            server.quarantineSession();
        }

        private void assertGeneration(long generation) {
            if (runtime.generation() != generation) throw new IllegalStateException("Stale Agent session generation");
        }

        private Object status() {
            java.util.Map<String, Object> result = new java.util.LinkedHashMap<>();
            result.put("generation", runtime.generation());
            result.put("state", runtime.state().name());
            result.put("activeOperations", runtime.activeOperationCount());
            return result;
        }

        private long generation() { return runtime.generation(); }
    }
}

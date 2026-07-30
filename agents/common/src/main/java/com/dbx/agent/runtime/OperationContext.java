package com.dbx.agent.runtime;

import com.google.gson.JsonObject;

public final class OperationContext {
    private final String operationId;
    private final String agentSessionId;
    private final long generation;
    private final Lane lane;

    public OperationContext(String operationId, String agentSessionId, long generation, Lane lane) {
        this.operationId = operationId;
        this.agentSessionId = agentSessionId;
        this.generation = generation;
        this.lane = lane;
    }

    public static OperationContext from(JsonObject params, String sessionId, long generation) {
        String id = params.has("operationId") ? params.get("operationId").getAsString() : java.util.UUID.randomUUID().toString();
        Lane lane = Lane.parse(params.has("lane") ? params.get("lane").getAsString() : null);
        return new OperationContext(id, sessionId, generation, lane);
    }

    public String operationId() { return operationId; }
    public String agentSessionId() { return agentSessionId; }
    public long generation() { return generation; }
    public Lane lane() { return lane; }
}

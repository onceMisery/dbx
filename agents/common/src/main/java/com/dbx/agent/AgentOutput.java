package com.dbx.agent;

/** Coordinates temporary driver stdout redirection with JSON-RPC protocol writes. */
public final class AgentOutput {
    public static final Object LOCK = new Object();

    private AgentOutput() { }
}

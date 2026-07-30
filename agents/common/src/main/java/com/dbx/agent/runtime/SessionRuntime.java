package com.dbx.agent.runtime;

import java.util.concurrent.atomic.AtomicReference;

public final class SessionRuntime {
    private final long generation;
    private final OperationRegistry operations;
    private final AtomicReference<SessionState> state = new AtomicReference<>(SessionState.ACTIVE);

    public SessionRuntime(long generation, OperationRegistry operations) {
        this.generation = generation;
        this.operations = operations;
    }

    public void assertDataRequestAllowed(long requestedGeneration) {
        if (requestedGeneration != generation) throw new IllegalStateException("Stale Agent session generation");
        if (state.get() != SessionState.ACTIVE) throw new IllegalStateException("Agent session is quarantined");
    }

    public boolean cancel(String operationId) {
        boolean accepted = operations.cancel(operationId);
        if (accepted) state.compareAndSet(SessionState.ACTIVE, SessionState.SUSPECT);
        return accepted;
    }

    public void quarantine() {
        state.set(SessionState.QUARANTINED);
        operations.cancelAll();
    }

    public void operationCompleted() {
        if (operations.activeCount() == 0) state.compareAndSet(SessionState.SUSPECT, SessionState.ACTIVE);
    }

    public void retiring() { state.set(SessionState.RETIRING); }
    public void retired() { state.set(SessionState.RETIRED); }
    public SessionState state() { return state.get(); }
    public long generation() { return generation; }
    public int activeOperationCount() { return operations.activeCount(); }
}

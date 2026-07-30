package com.dbx.agent.runtime;

import java.sql.SQLException;
import java.sql.Statement;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.atomic.AtomicReference;

public final class OperationRegistry {
    private static final ThreadPoolExecutor CANCELLATIONS = new ThreadPoolExecutor(
        2,
        2,
        0L,
        TimeUnit.MILLISECONDS,
        new ArrayBlockingQueue<>(64),
        runnable -> {
            Thread thread = new Thread(runnable, "agent-statement-cancel");
            thread.setDaemon(true);
            return thread;
        },
        new ThreadPoolExecutor.AbortPolicy()
    );
    private final Map<String, Entry> operations = new ConcurrentHashMap<>();

    public void start(OperationContext context) {
        Entry entry = new Entry();
        if (operations.putIfAbsent(context.operationId(), entry) != null) throw new IllegalStateException("Operation already exists");
        entry.state.set(OperationState.RUNNING);
    }

    public AutoCloseable registerStatement(String operationId, Statement statement) {
        Entry entry = required(operationId);
        entry.statements.put(statement, Boolean.TRUE);
        return () -> {
            entry.statements.remove(statement);
            removeCompletedEntry(operationId, entry);
        };
    }

    public boolean cancel(String operationId) {
        Entry entry = operations.get(operationId);
        if (entry == null || entry.state.get().isTerminal()) return false;
        entry.state.compareAndSet(OperationState.RUNNING, OperationState.CANCEL_REQUESTED);
        for (Statement statement : entry.statements.keySet()) {
            try {
                CANCELLATIONS.execute(() -> {
                    try { statement.cancel(); } catch (SQLException ignored) { }
                });
            } catch (RejectedExecutionException ignored) {
                // Quarantine still removes the Session from routing. A saturated
                // cancel executor is handled by the client process-replacement threshold.
            }
        }
        return true;
    }

    public void finish(String operationId, OperationState terminal) {
        if (!terminal.isTerminal()) throw new IllegalArgumentException("Terminal state required");
        Entry entry = operations.get(operationId);
        if (entry == null) return;
        OperationState current = entry.state.get();
        if (!current.isTerminal()) entry.state.compareAndSet(current, terminal);
        removeCompletedEntry(operationId, entry);
    }

    public OperationState state(String operationId) {
        Entry entry = operations.get(operationId);
        return entry == null ? null : entry.state.get();
    }

    public int activeCount() { return operations.size(); }
    public void cancelAll() { for (String id : operations.keySet()) cancel(id); }

    private Entry required(String id) {
        Entry entry = operations.get(id);
        if (entry == null) throw new IllegalStateException("Operation not registered: " + id);
        return entry;
    }

    private void removeCompletedEntry(String operationId, Entry entry) {
        if (entry.state.get().isTerminal() && entry.statements.isEmpty()) {
            operations.remove(operationId, entry);
        }
    }

    private static final class Entry {
        private final AtomicReference<OperationState> state = new AtomicReference<>(OperationState.CREATED);
        private final Map<Statement, Boolean> statements = new ConcurrentHashMap<>();
    }
}

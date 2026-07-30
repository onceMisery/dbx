package com.dbx.agent.jdbc;

import java.util.concurrent.atomic.AtomicBoolean;

/** Idempotent handle for removing a statement from every cancellation registry. */
public final class StatementRegistration implements AutoCloseable {
    private final Runnable unregister;
    private final AtomicBoolean closed = new AtomicBoolean();

    public StatementRegistration(Runnable unregister) {
        this.unregister = unregister;
    }

    @Override
    public void close() {
        if (closed.compareAndSet(false, true)) {
            unregister.run();
        }
    }
}

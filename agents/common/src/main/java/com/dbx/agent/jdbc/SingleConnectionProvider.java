package com.dbx.agent.jdbc;

import java.sql.Connection;
import java.time.Duration;
import java.util.concurrent.Executor;
import java.util.concurrent.Semaphore;
import java.util.concurrent.atomic.AtomicBoolean;

/** Reference provider for compatibility and contract tests; Dameng uses Hikari in production. */
public final class SingleConnectionProvider implements ConnectionProvider {
    private static final Executor DIRECT_EXECUTOR = Runnable::run;

    private final ConnectionFactory factory;
    private final Semaphore ownership = new Semaphore(1, true);
    private final AtomicBoolean closed = new AtomicBoolean();
    private Connection connection;

    public SingleConnectionProvider(ConnectionFactory factory) {
        this.factory = factory;
    }

    @Override
    public ConnectionLease acquire(Duration timeout) throws Exception {
        if (closed.get()) throw new IllegalStateException("Connection provider is closed");
        if (!ownership.tryAcquire(timeout.toMillis(), java.util.concurrent.TimeUnit.MILLISECONDS)) {
            throw new IllegalStateException("Connection checkout timed out");
        }
        try {
            if (closed.get()) throw new IllegalStateException("Connection provider is closed");
            if (connection == null || connection.isClosed()) connection = factory.open();
            return new Lease(connection);
        } catch (Exception error) {
            ownership.release();
            throw error;
        }
    }

    @Override
    public boolean isClosed() {
        return closed.get();
    }

    @Override
    public void close() {
        if (!closed.compareAndSet(false, true)) return;
        Connection current = connection;
        connection = null;
        if (current != null) abortAndClose(current);
    }

    private static void abortAndClose(Connection target) {
        try { target.abort(DIRECT_EXECUTOR); } catch (Exception ignored) {
            try { target.close(); } catch (Exception ignoredClose) { }
        }
    }

    private final class Lease implements ConnectionLease {
        private final Connection leased;
        private final AtomicBoolean finished = new AtomicBoolean();
        private final AtomicBoolean invalidated = new AtomicBoolean();

        private Lease(Connection leased) { this.leased = leased; }

        @Override public Connection connection() { return leased; }

        @Override
        public void invalidate(LeaseInvalidationCause cause) {
            if (!invalidated.compareAndSet(false, true)) return;
            if (connection == leased) connection = null;
            abortAndClose(leased);
        }

        @Override public boolean isInvalidated() { return invalidated.get(); }

        @Override
        public void close() {
            if (!finished.compareAndSet(false, true)) return;
            if (closed.get() && !invalidated.get()) invalidate(LeaseInvalidationCause.PROVIDER_CLOSED);
            ownership.release();
        }
    }
}

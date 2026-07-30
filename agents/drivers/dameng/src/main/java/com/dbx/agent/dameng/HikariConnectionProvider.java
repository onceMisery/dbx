package com.dbx.agent.dameng;

import com.dbx.agent.jdbc.ConnectionLease;
import com.dbx.agent.jdbc.ConnectionProvider;
import com.dbx.agent.jdbc.LeaseInvalidationCause;
import com.zaxxer.hikari.HikariDataSource;
import java.sql.Connection;
import java.time.Duration;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.RejectedExecutionException;

final class HikariConnectionProvider implements ConnectionProvider {
    private static final ArrayBlockingQueue<Runnable> DISPOSAL_OVERFLOW = new ArrayBlockingQueue<>(512);
    private static final ExecutorService DISPOSAL = new ThreadPoolExecutor(
        2,
        2,
        0L,
        TimeUnit.MILLISECONDS,
        new ArrayBlockingQueue<>(128),
        runnable -> {
            Thread thread = new Thread(runnable, "dameng-connection-disposal");
            thread.setDaemon(true);
            return thread;
        },
        new ThreadPoolExecutor.AbortPolicy()
    );
    static {
        Thread overflowWorker = new Thread(() -> {
            while (!Thread.currentThread().isInterrupted()) {
                try {
                    DISPOSAL_OVERFLOW.take().run();
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                } catch (Throwable ignored) {
                    // Individual cleanup failures must not terminate the only overflow worker.
                }
            }
        }, "dameng-connection-disposal-overflow");
        overflowWorker.setDaemon(true);
        overflowWorker.start();
    }
    private final HikariDataSource dataSource;
    private final AtomicBoolean closed = new AtomicBoolean();
    private final Set<Lease> activeLeases = ConcurrentHashMap.newKeySet();
    private final Object lifecycleLock = new Object();
    private final Object checkoutLock = new Object();

    HikariConnectionProvider(HikariDataSource dataSource) {
        this.dataSource = dataSource;
    }

    @Override
    public ConnectionLease acquire(Duration timeout) throws Exception {
        if (closed.get()) throw new IllegalStateException("Connection provider is closed");
        Connection connection;
        synchronized (checkoutLock) {
            if (closed.get()) throw new IllegalStateException("Connection provider is closed");
            long timeoutMillis = Math.max(250L, timeout.toMillis());
            dataSource.getHikariConfigMXBean().setConnectionTimeout(timeoutMillis);
            connection = dataSource.getConnection();
        }
        Lease lease = new Lease(connection);
        synchronized (lifecycleLock) {
            if (!closed.get()) {
                activeLeases.add(lease);
                return lease;
            }
        }
        lease.invalidateNow();
        throw new IllegalStateException("Connection provider closed during checkout");
    }

    @Override public boolean isClosed() { return closed.get(); }

    @Override
    public void close() {
        Lease[] leases;
        synchronized (lifecycleLock) {
            if (!closed.compareAndSet(false, true)) return;
            leases = activeLeases.toArray(new Lease[0]);
        }
        submitDisposal(() -> {
            for (Lease lease : leases) lease.invalidateNow();
            dataSource.close();
        });
    }

    private final class Lease implements ConnectionLease {
        private final Connection connection;
        private final AtomicBoolean completed = new AtomicBoolean();
        private final AtomicBoolean invalidated = new AtomicBoolean();

        private Lease(Connection connection) { this.connection = connection; }
        @Override public Connection connection() { return connection; }

        @Override
        public void invalidate(LeaseInvalidationCause cause) {
            if (!invalidated.compareAndSet(false, true)) return;
            submitDisposal(this::evict);
        }

        private void invalidateNow() {
            if (invalidated.compareAndSet(false, true)) evict();
        }

        private void evict() {
            try { dataSource.evictConnection(connection); } catch (Exception ignored) { }
            activeLeases.remove(this);
        }

        @Override public boolean isInvalidated() { return invalidated.get(); }

        @Override
        public void close() {
            if (!completed.compareAndSet(false, true)) return;
            if (closed.get() && !invalidated.get()) invalidate(LeaseInvalidationCause.PROVIDER_CLOSED);
            // An invalidated proxy must stay checked out until eviction owns it.
            // Returning it normally would create a window where unknown state is borrowed again.
            if (invalidated.get()) return;
            try { connection.close(); } catch (Exception ignored) { }
            activeLeases.remove(this);
        }
    }

    private static void submitDisposal(Runnable task) {
        try {
            DISPOSAL.execute(task);
        } catch (RejectedExecutionException saturated) {
            if (!DISPOSAL_OVERFLOW.offer(task)) {
                throw new IllegalStateException("Dameng connection disposal capacity exhausted", saturated);
            }
        }
    }
}

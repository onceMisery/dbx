package com.dbx.agent.jdbc;

import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.lang.reflect.Proxy;
import java.sql.Connection;
import java.time.Duration;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

class SingleConnectionProviderTest {
    @Test
    void activeLeaseEnforcesExclusiveCheckout() throws Exception {
        SingleConnectionProvider provider = new SingleConnectionProvider(() -> connection(new AtomicBoolean()));
        try (ConnectionLease ignored = provider.acquire(Duration.ofSeconds(1))) {
            assertThrows(IllegalStateException.class, () -> provider.acquire(Duration.ofMillis(10)));
        } finally {
            provider.close();
        }
    }

    @Test
    void invalidatedConnectionIsNeverBorrowedAgain() throws Exception {
        AtomicInteger opened = new AtomicInteger();
        AtomicBoolean firstAborted = new AtomicBoolean();
        SingleConnectionProvider provider = new SingleConnectionProvider(() -> connection(opened.incrementAndGet() == 1 ? firstAborted : new AtomicBoolean()));

        Connection first;
        try (ConnectionLease lease = provider.acquire(Duration.ofSeconds(1))) {
            first = lease.connection();
            lease.invalidate(LeaseInvalidationCause.QUERY_STATE_UNKNOWN);
        }
        try (ConnectionLease lease = provider.acquire(Duration.ofSeconds(1))) {
            assertNotSame(first, lease.connection());
        }

        assertTrue(firstAborted.get());
        provider.close();
        assertThrows(IllegalStateException.class, () -> provider.acquire(Duration.ofMillis(10)));
    }

    private static Connection connection(AtomicBoolean aborted) {
        return (Connection) Proxy.newProxyInstance(
            SingleConnectionProviderTest.class.getClassLoader(),
            new Class<?>[]{Connection.class},
            (proxy, method, args) -> {
                if ("abort".equals(method.getName())) { aborted.set(true); return null; }
                if ("isClosed".equals(method.getName())) return aborted.get();
                if (method.getReturnType() == boolean.class) return false;
                if (method.getReturnType() == int.class) return 0;
                return null;
            }
        );
    }
}

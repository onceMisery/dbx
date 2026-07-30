package com.dbx.agent.dameng;

import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertThrows;

import com.dbx.agent.jdbc.ConnectionLease;
import com.dbx.agent.jdbc.LeaseInvalidationCause;
import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;
import java.sql.Connection;
import java.time.Duration;
import org.junit.jupiter.api.Test;

class HikariConnectionProviderTest {
    @Test
    void invalidatedPhysicalConnectionIsNeverBorrowedAgain() throws Exception {
        HikariConnectionProvider provider = provider();
        Connection firstPhysical;
        try (ConnectionLease lease = provider.acquire(Duration.ofSeconds(1))) {
            firstPhysical = lease.connection().unwrap(org.h2.jdbc.JdbcConnection.class);
            lease.invalidate(LeaseInvalidationCause.QUERY_STATE_UNKNOWN);
        }

        Connection replacementPhysical;
        try (ConnectionLease lease = provider.acquire(Duration.ofSeconds(1))) {
            replacementPhysical = lease.connection().unwrap(org.h2.jdbc.JdbcConnection.class);
        }

        assertNotSame(firstPhysical, replacementPhysical);
        provider.close();
        assertThrows(IllegalStateException.class, () -> provider.acquire(Duration.ofMillis(250)));
    }

    @Test
    void activeLeaseEnforcesExclusiveCheckout() throws Exception {
        HikariConnectionProvider provider = provider();
        try (ConnectionLease ignored = provider.acquire(Duration.ofSeconds(1))) {
            assertThrows(Exception.class, () -> provider.acquire(Duration.ofMillis(250)));
        } finally {
            provider.close();
        }
    }

    private static HikariConnectionProvider provider() {
        HikariConfig config = new HikariConfig();
        config.setJdbcUrl("jdbc:h2:mem:provider-contract-" + System.nanoTime());
        config.setMinimumIdle(0);
        config.setMaximumPoolSize(1);
        config.setConnectionTimeout(250L);
        return new HikariConnectionProvider(new HikariDataSource(config));
    }

}

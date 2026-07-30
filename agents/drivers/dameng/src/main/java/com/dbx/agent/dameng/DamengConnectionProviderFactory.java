package com.dbx.agent.dameng;

import com.dbx.agent.ConnectParams;
import com.dbx.agent.jdbc.ConnectionProvider;
import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;

final class DamengConnectionProviderFactory {
    private DamengConnectionProviderFactory() { }

    static ConnectionProvider create(String jdbcUrl, ConnectParams params) {
        HikariConfig config = new HikariConfig();
        config.setPoolName("dbx-dameng-session-" + Integer.toHexString(System.identityHashCode(params)));
        config.setDriverClassName("dm.jdbc.driver.DmDriver");
        config.setJdbcUrl(jdbcUrl);
        config.setUsername(params.getUsername());
        config.setPassword(params.getPassword());
        config.setMinimumIdle(0);
        // One physical connection owns all connection-scoped state for this
        // Session generation. Metadata and workload isolation use separate Sessions.
        config.setMaximumPoolSize(1);
        config.setConnectionTimeout(30_000L);
        config.setValidationTimeout(5_000L);
        config.setIdleTimeout(600_000L);
        config.setMaxLifetime(1_800_000L);
        config.setKeepaliveTime(30_000L);
        return new HikariConnectionProvider(new HikariDataSource(config));
    }
}

package com.dbx.agent.dameng;

import com.dbx.agent.JdbcExecutor;
import com.dbx.agent.jdbc.PreparedQuery;
import com.dbx.agent.jdbc.RowMapper;
import java.sql.Connection;
import java.sql.Statement;
import java.lang.reflect.Method;
import java.util.List;
import java.util.Optional;
import java.util.function.Supplier;

/** Dameng catalog access. Connection ownership and JDBC execution stay in their dedicated layers. */
final class DamengMetadataRepository {
    private static final int METADATA_TIMEOUT_SECS = 60;

    private final Supplier<Connection> connectionSupplier;

    DamengMetadataRepository(Supplier<Connection> connectionSupplier) {
        this.connectionSupplier = connectionSupplier;
    }

    <T> List<T> query(String sql, List<?> arguments, RowMapper<T> mapper) {
        return JdbcExecutor.current().query(
            connectionSupplier.get(),
            new PreparedQuery(sql, arguments),
            METADATA_TIMEOUT_SECS,
            mapper
        );
    }

    <T> List<T> query(String sql, RowMapper<T> mapper, Object... arguments) {
        return query(sql, arguments == null ? List.of() : List.of(arguments), mapper);
    }

    <T> Optional<T> queryOne(String sql, RowMapper<T> mapper, Object... arguments) {
        List<T> rows = query(sql, mapper, arguments);
        return rows.isEmpty() ? Optional.empty() : Optional.ofNullable(rows.get(0));
    }

    void execute(String sql, int timeoutSecs) {
        JdbcExecutor.current().executeStatement(connectionSupplier.get(), sql, timeoutSecs);
    }

    String tracePlan(String sql, int timeoutSecs) {
        return JdbcExecutor.current().executeObserved(
            connectionSupplier.get(),
            sql,
            timeoutSecs,
            (connection, statement) -> invokePlan(connection, statement)
        );
    }

    String driverPlan(String sql) {
        Connection connection = connectionSupplier.get();
        try {
            Class<?> type = Class.forName("dm.jdbc.driver.DmdbConnection");
            if (!type.isInstance(connection)) return null;
            Method method = type.getMethod("getExplainInfo", String.class);
            return (String) method.invoke(type.cast(connection), sql);
        } catch (ReflectiveOperationException ignored) {
            return null;
        }
    }

    String fallbackPlan(String sql) {
        return String.join("\n", JdbcExecutor.current().queryStatement(
            connectionSupplier.get(),
            "EXPLAIN " + sql,
            METADATA_TIMEOUT_SECS,
            resultSet -> resultSet.getString(1)
        ));
    }

    private static String invokePlan(Connection connection, Statement statement) {
        try {
            Class<?> type = Class.forName("dm.jdbc.driver.DmdbConnection");
            if (!type.isInstance(connection)) return null;
            Method method = type.getMethod("getExplainInfo", Statement.class);
            return (String) method.invoke(type.cast(connection), statement);
        } catch (ReflectiveOperationException ignored) {
            return null;
        }
    }
}

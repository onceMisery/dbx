package com.dbx.agent.jdbc;

import java.util.List;

/** Immutable SQL and bind values passed to the JDBC execution boundary. */
public record PreparedQuery(String sql, List<?> arguments) {
    public PreparedQuery {
        if (sql == null || sql.isBlank()) {
            throw new IllegalArgumentException("SQL must not be blank");
        }
        arguments = arguments == null ? List.of() : List.copyOf(arguments);
    }

    public static PreparedQuery of(String sql, Object... arguments) {
        return new PreparedQuery(sql, arguments == null ? List.of() : List.of(arguments));
    }
}

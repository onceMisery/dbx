package com.dbx.agent.jdbc;

import java.sql.Connection;
import java.sql.SQLException;
import java.sql.Statement;

@FunctionalInterface
public interface ExecutedStatementMapper<T> {
    T map(Connection connection, Statement statement) throws Exception;
}

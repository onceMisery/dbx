package com.dbx.agent.jdbc;

import java.sql.ResultSet;

@FunctionalInterface
public interface RowMapper<T> {
    T map(ResultSet resultSet) throws Exception;
}

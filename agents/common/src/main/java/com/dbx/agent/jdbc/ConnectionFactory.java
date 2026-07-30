package com.dbx.agent.jdbc;

import java.sql.Connection;

@FunctionalInterface
public interface ConnectionFactory {
    Connection open() throws Exception;
}

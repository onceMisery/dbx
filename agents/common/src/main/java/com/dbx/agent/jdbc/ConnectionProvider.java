package com.dbx.agent.jdbc;

import java.time.Duration;

public interface ConnectionProvider extends AutoCloseable {
    ConnectionLease acquire(Duration timeout) throws Exception;

    boolean isClosed();

    @Override
    void close();
}

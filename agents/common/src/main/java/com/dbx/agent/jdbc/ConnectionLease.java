package com.dbx.agent.jdbc;

import java.sql.Connection;

public interface ConnectionLease extends AutoCloseable {
    Connection connection();

    void invalidate(LeaseInvalidationCause cause);

    boolean isInvalidated();

    @Override
    void close();
}

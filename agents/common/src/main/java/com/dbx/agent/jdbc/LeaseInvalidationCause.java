package com.dbx.agent.jdbc;

public enum LeaseInvalidationCause {
    QUERY_STATE_UNKNOWN,
    CONNECTION_BROKEN,
    RESET_FAILED,
    SESSION_QUARANTINED,
    PROVIDER_CLOSED
}

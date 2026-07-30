package com.dbx.agent.runtime;

public enum Lane {
    METADATA, WORKLOAD;

    public static Lane parse(String value) {
        return value == null ? WORKLOAD : valueOf(value.trim().toUpperCase(java.util.Locale.ROOT));
    }
}

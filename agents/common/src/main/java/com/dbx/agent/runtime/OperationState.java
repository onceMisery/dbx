package com.dbx.agent.runtime;

public enum OperationState {
    CREATED, RUNNING, CANCEL_REQUESTED, SUCCEEDED, FAILED, CANCELLED_CONFIRMED, ABANDONED;

    public boolean isTerminal() {
        return this == SUCCEEDED || this == FAILED || this == CANCELLED_CONFIRMED || this == ABANDONED;
    }
}

package com.dbx.agent;

import com.dbx.agent.runtime.OperationContext;
import com.dbx.agent.runtime.OperationRegistry;

final class AgentExecutionContext {
    private static final ThreadLocal<JdbcExecutor> JDBC_EXECUTOR = new ThreadLocal<>();
    private static final ThreadLocal<OperationContext> OPERATION_CONTEXT = new ThreadLocal<>();
    private static final ThreadLocal<OperationRegistry> OPERATION_REGISTRY = new ThreadLocal<>();

    private AgentExecutionContext() {
    }

    static JdbcExecutor jdbcExecutor() {
        JdbcExecutor executor = JDBC_EXECUTOR.get();
        return executor == null ? JdbcExecutor.INSTANCE : executor;
    }

    static <T> T withJdbcExecutor(JdbcExecutor executor, DatabaseAgent.ThrowingSupplier<T> supplier) throws Exception {
        return withJdbcExecutor(executor, null, null, supplier);
    }

    static void runWithJdbcExecutor(JdbcExecutor executor, Runnable action) {
        try {
            withJdbcExecutor(executor, () -> {
                action.run();
                return null;
            });
        } catch (RuntimeException error) {
            throw error;
        } catch (Exception impossible) {
            throw new IllegalStateException("JDBC executor lifecycle callback failed", impossible);
        }
    }

    static <T> T withJdbcExecutor(
        JdbcExecutor executor,
        OperationContext operationContext,
        OperationRegistry operationRegistry,
        DatabaseAgent.ThrowingSupplier<T> supplier
    ) throws Exception {
        JdbcExecutor previous = JDBC_EXECUTOR.get();
        OperationContext previousContext = OPERATION_CONTEXT.get();
        OperationRegistry previousRegistry = OPERATION_REGISTRY.get();
        JDBC_EXECUTOR.set(executor);
        if (operationContext != null) OPERATION_CONTEXT.set(operationContext);
        if (operationRegistry != null) OPERATION_REGISTRY.set(operationRegistry);
        try {
            return supplier.get();
        } finally {
            if (previous == null) {
                JDBC_EXECUTOR.remove();
            } else {
                JDBC_EXECUTOR.set(previous);
            }
            restore(OPERATION_CONTEXT, previousContext);
            restore(OPERATION_REGISTRY, previousRegistry);
        }
    }

    static AutoCloseable registerStatement(java.sql.Statement statement) {
        OperationContext context = OPERATION_CONTEXT.get();
        OperationRegistry registry = OPERATION_REGISTRY.get();
        if (context == null || registry == null) return () -> { };
        return registry.registerStatement(context.operationId(), statement);
    }

    private static <T> void restore(ThreadLocal<T> local, T previous) {
        if (previous == null) local.remove(); else local.set(previous);
    }
}

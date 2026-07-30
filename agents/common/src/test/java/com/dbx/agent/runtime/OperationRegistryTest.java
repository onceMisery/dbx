package com.dbx.agent.runtime;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.lang.reflect.Proxy;
import java.sql.Statement;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;

class OperationRegistryTest {
    @Test
    void twentyBlockingDriverCancelsDoNotBlockTheControlThread() throws Exception {
        OperationRegistry registry = new OperationRegistry();
        CountDownLatch cancelWorkersEntered = new CountDownLatch(2);
        CountDownLatch releaseDrivers = new CountDownLatch(1);
        List<AutoCloseable> registrations = new ArrayList<>();
        try {
            long started = System.nanoTime();
            for (int index = 0; index < 20; index++) {
                String operationId = "blocked-" + index;
                registry.start(new OperationContext(operationId, "session", 1L, Lane.WORKLOAD));
                registrations.add(registry.registerStatement(
                    operationId,
                    blockingStatement(cancelWorkersEntered, releaseDrivers)
                ));
                assertTrue(registry.cancel(operationId));
            }

            assertTrue(cancelWorkersEntered.await(1, TimeUnit.SECONDS));
            assertTrue(
                System.nanoTime() - started < TimeUnit.SECONDS.toNanos(1),
                "control calls must not wait for a stuck JDBC cancel"
            );
            assertEquals(20, registry.activeCount());
        } finally {
            releaseDrivers.countDown();
            for (int index = 0; index < registrations.size(); index++) {
                registry.finish("blocked-" + index, OperationState.ABANDONED);
                registrations.get(index).close();
            }
        }
        assertEquals(0, registry.activeCount());
    }

    @Test
    void cancelTargetsOnlyTheRequestedOperationAndTerminalStateIsIrreversible() throws Exception {
        OperationRegistry registry = new OperationRegistry();
        registry.start(new OperationContext("one", "session", 1L, Lane.WORKLOAD));
        registry.start(new OperationContext("two", "session", 1L, Lane.WORKLOAD));
        AtomicBoolean firstCancelled = new AtomicBoolean();
        AtomicBoolean secondCancelled = new AtomicBoolean();
        Statement first = statement(firstCancelled);
        Statement second = statement(secondCancelled);

        try (AutoCloseable ignoredFirst = registry.registerStatement("one", first);
             AutoCloseable ignoredSecond = registry.registerStatement("two", second)) {
            assertTrue(registry.cancel("one"));
            assertTrue(awaitTrue(firstCancelled));
            assertFalse(secondCancelled.get());
            assertEquals(OperationState.CANCEL_REQUESTED, registry.state("one"));
            registry.finish("one", OperationState.ABANDONED);
            registry.finish("one", OperationState.SUCCEEDED);
            assertEquals(OperationState.ABANDONED, registry.state("one"));
        }
        assertEquals(null, registry.state("one"));
        assertEquals(1, registry.activeCount());
    }

    private static boolean awaitTrue(AtomicBoolean value) throws InterruptedException {
        long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.SECONDS.toNanos(1);
        while (!value.get() && System.nanoTime() < deadline) Thread.sleep(5L);
        return value.get();
    }

    private static Statement statement(AtomicBoolean cancelled) {
        return (Statement) Proxy.newProxyInstance(
            OperationRegistryTest.class.getClassLoader(),
            new Class<?>[]{Statement.class},
            (proxy, method, args) -> {
                if ("cancel".equals(method.getName())) {
                    cancelled.set(true);
                    return null;
                }
                Class<?> type = method.getReturnType();
                if (type == boolean.class) return false;
                if (type == int.class) return 0;
                if (type == long.class) return 0L;
                return null;
            }
        );
    }

    private static Statement blockingStatement(CountDownLatch entered, CountDownLatch release) {
        return (Statement) Proxy.newProxyInstance(
            OperationRegistryTest.class.getClassLoader(),
            new Class<?>[]{Statement.class},
            (proxy, method, args) -> {
                if ("cancel".equals(method.getName())) {
                    entered.countDown();
                    release.await();
                    return null;
                }
                Class<?> type = method.getReturnType();
                if (type == boolean.class) return false;
                if (type == int.class) return 0;
                if (type == long.class) return 0L;
                return null;
            }
        );
    }
}

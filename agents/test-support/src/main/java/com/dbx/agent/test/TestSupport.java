package com.dbx.agent.test;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.sql.Connection;

public final class TestSupport {
    private TestSupport() {
    }

    public static void setPrivateConnection(Object target, Connection connection) {
        try {
            Method seam = findConnectionSeam(target.getClass());
            if (seam != null) {
                seam.setAccessible(true);
                seam.invoke(target, connection);
                return;
            }
            Field field = findConnectionField(target.getClass());
            field.setAccessible(true);
            field.set(target, connection);
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("Unable to set connection", e);
        }
    }

    private static Method findConnectionSeam(Class<?> type) {
        Class<?> current = type;
        while (current != null) {
            try {
                return current.getDeclaredMethod("setConnectionForTest", Connection.class);
            } catch (NoSuchMethodException ignored) {
                current = current.getSuperclass();
            }
        }
        return null;
    }

    private static Field findConnectionField(Class<?> type) {
        Class<?> current = type;
        while (current != null) {
            try {
                return current.getDeclaredField("connection");
            } catch (NoSuchFieldException ignored) {
                current = current.getSuperclass();
            }
        }
        throw new IllegalStateException(new NoSuchFieldException("connection"));
    }
}

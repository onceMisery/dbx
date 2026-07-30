package com.dbx.agent.dameng;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertFalse;

class DamengArchitectureTest {
    private static final Path SOURCE_ROOT = Path.of("src/main/java/com/dbx/agent/dameng");

    @Test
    void agentAndRepositoryCannotExecuteJdbcDirectly() throws IOException {
        for (String file : List.of("DamengAgent.java", "DamengMetadataRepository.java")) {
            String source = Files.readString(SOURCE_ROOT.resolve(file));
            assertFalse(source.contains("prepareStatement("), file + " must delegate prepared queries to JdbcExecutor");
            assertFalse(source.contains("createStatement("), file + " must delegate statement creation to JdbcExecutor");
            assertFalse(source.contains(".executeQuery("), file + " must delegate query execution to JdbcExecutor");
        }
    }

    @Test
    void agentDoesNotDependOnJdbcStatementImplementations() throws IOException {
        String source = Files.readString(SOURCE_ROOT.resolve("DamengAgent.java"));
        assertFalse(source.contains("import java.sql.PreparedStatement;"));
        assertFalse(source.contains("import java.sql.Statement;"));
        assertFalse(source.contains("DriverManager"));
        assertFalse(source.contains("registerExternalStatement("));
    }
}

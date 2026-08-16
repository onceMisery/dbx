package com.dbx.agent.sqlserverlegacy;

import com.dbx.agent.ConnectParams;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.Test;

class SqlServer2008AgentTest {
    @Test
    void usesTheMicrosoftDriverVersionThatSupportsSqlServer2008() {
        Assertions.assertEquals(6, SqlServerLegacyAgent.jdbcDriverMajorVersion());
        Assertions.assertEquals(2, SqlServerLegacyAgent.jdbcDriverMinorVersion());
        Assertions.assertFalse(SqlServerLegacyAgent.jdbcSupportsSslProtocolProperty());
    }

    @Test
    void omitsTheUnsupportedSslProtocolDefault() {
        String url = SqlServerLegacyAgent.legacyTlsUrl(connectParams("applicationName=dbx"));

        Assertions.assertEquals(
            "jdbc:sqlserver://db.example.com:1433;databaseName=master;applicationName=dbx;encrypt=true;trustServerCertificate=true",
            url
        );
    }

    @Test
    void rejectsAnExplicitSslProtocolThatJdbc62CannotHonor() {
        IllegalArgumentException error = Assertions.assertThrows(
            IllegalArgumentException.class,
            () -> SqlServerLegacyAgent.legacyTlsUrl(connectParams("sslProtocol=TLSv1.2"))
        );

        Assertions.assertTrue(error.getMessage().contains("mssql-jdbc 6.2"));
        Assertions.assertTrue(error.getMessage().contains("sslProtocol"));
    }

    private static ConnectParams connectParams(String urlParams) {
        ConnectParams params = new ConnectParams();
        params.setHost("db.example.com");
        params.setPort(1433);
        params.setPort_explicit(true);
        params.setDatabase("master");
        params.setUsername("sa");
        params.setPassword("secret");
        params.setUrl_params(urlParams);
        return params;
    }
}

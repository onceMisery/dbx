package com.dbx.agent.sqlserver2008;

import com.dbx.agent.MultiSessionJsonRpcServer;
import com.dbx.agent.sqlserverlegacy.SqlServerLegacyAgent;

public final class SqlServer2008Agent extends SqlServerLegacyAgent {
    public static void main(String[] args) throws Exception {
        new MultiSessionJsonRpcServer(SqlServer2008Agent::new).run();
    }
}

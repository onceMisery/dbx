import { describe, expect, it } from "vitest";
import {
  isSqlServerNativeEncryptionDisabled,
  isSqlServerLegacyTlsUnsupportedFailure,
  isSqlServerTlsHandshakeFailure,
  migrateSqlServerLegacyCompatibilityConfig,
  requiredSqlServerCompatibilityDriverKey,
  requiresSqlServerLegacyCompatibilityComponent,
  setSqlServerDriverModeConfig,
  setSqlServerLegacyCompatibilityConfig,
  setSqlServerNativeEncryptionDisabled,
  sqlServerUsesLegacyCompatibility,
  sqlServerUses2008Driver,
} from "@/lib/connection/sqlServerLegacyCompatibility";
import type { ConnectionConfig } from "@/types/database";

function connectionConfig(urlParams?: string): ConnectionConfig {
  return {
    id: "sqlserver",
    name: "SQL Server",
    db_type: "sqlserver",
    driver_profile: "sqlserver",
    driver_label: "SQL Server",
    host: "127.0.0.1",
    port: 1433,
    username: "sa",
    password: "secret",
    database: "master",
    url_params: urlParams,
    ssl: false,
    ssh_enabled: false,
    read_only: false,
    one_time: false,
    transport_layers: [],
    agent_java_options: [],
  };
}

describe("SQL Server legacy compatibility", () => {
  it("recognizes native encryption policy independently from the legacy driver profile", () => {
    expect(isSqlServerNativeEncryptionDisabled("sqlserverEncryption=disabled")).toBe(true);
    expect(isSqlServerNativeEncryptionDisabled("applicationName=dbx;encrypt=false")).toBe(true);
    expect(isSqlServerNativeEncryptionDisabled("?Encrypt=0&applicationName=dbx")).toBe(true);
    expect(isSqlServerNativeEncryptionDisabled("encrypt=true")).toBe(false);
  });

  it("updates native encryption params without changing the driver profile", () => {
    expect(setSqlServerNativeEncryptionDisabled("applicationName=dbx;encrypt=true", true)).toBe("applicationName=dbx&sqlserverEncryption=disabled");
    expect(setSqlServerNativeEncryptionDisabled("applicationName=dbx;sqlserverEncryption=disabled", false)).toBe("applicationName=dbx");
  });

  it("keeps historical disabled-encryption connections on the native driver profile", () => {
    const config = connectionConfig("sqlserverEncryption=disabled");
    migrateSqlServerLegacyCompatibilityConfig(config);

    expect(sqlServerUsesLegacyCompatibility(config)).toBe(false);
    expect(requiresSqlServerLegacyCompatibilityComponent(config)).toBe(false);
    expect(config.driver_label).toBe("SQL Server");
    expect(config.url_params).toBe("sqlserverEncryption=disabled");
    expect(
      requiresSqlServerLegacyCompatibilityComponent({
        ...config,
        driver_profile: "sqlserver-legacy",
        db_type: "mysql",
      }),
    ).toBe(false);
  });

  it("preserves unrelated params with the historical compatibility flag", () => {
    const config = connectionConfig("applicationName=dbx;sqlserverEncryption=off;encrypt=false");

    migrateSqlServerLegacyCompatibilityConfig(config);

    expect(config.driver_profile).toBe("sqlserver");
    expect(config.url_params).toBe("applicationName=dbx;sqlserverEncryption=off;encrypt=false");
  });

  it("preserves semicolons and special characters inside braced values", () => {
    const config = connectionConfig("applicationName={DBX; Client};password=50%;sqlserverEncryption=disabled;encrypt=false");

    migrateSqlServerLegacyCompatibilityConfig(config);

    expect(config.driver_profile).toBe("sqlserver");
    expect(config.url_params).toBe("applicationName={DBX; Client};password=50%;sqlserverEncryption=disabled;encrypt=false");
  });

  it("keeps escaped closing braces from exposing separators inside braced values", () => {
    const config = connectionConfig("applicationName={DBX}}; Client};sqlserverEncryption=disabled;encrypt=false");

    migrateSqlServerLegacyCompatibilityConfig(config);

    expect(config.url_params).toBe("applicationName={DBX}}; Client};sqlserverEncryption=disabled;encrypt=false");
  });

  it("keeps generic JDBC encrypt=false on the native driver", () => {
    const config = connectionConfig("applicationName=dbx;encrypt=false");

    migrateSqlServerLegacyCompatibilityConfig(config);

    expect(config.driver_profile).toBe("sqlserver");
    expect(config.url_params).toBe("applicationName=dbx;encrypt=false");
  });

  it("treats a persisted legacy driver profile as compatibility mode", () => {
    const config = connectionConfig("");
    config.driver_profile = "sqlserver-legacy";

    expect(sqlServerUsesLegacyCompatibility(config)).toBe(true);
    expect(requiresSqlServerLegacyCompatibilityComponent(config)).toBe(true);
  });

  it("updates the explicit driver profile without rewriting native encryption params", () => {
    const config = connectionConfig("applicationName=dbx&encrypt=false");

    setSqlServerLegacyCompatibilityConfig(config, true);
    expect(config.driver_profile).toBe("sqlserver-legacy");
    expect(config.driver_label).toBe("SQL Server TLS 1.0 Compatibility Driver");
    expect(config.url_params).toBe("applicationName=dbx&encrypt=false");

    setSqlServerLegacyCompatibilityConfig(config, false);
    expect(config.driver_profile).toBe("sqlserver");
    expect(config.driver_label).toBe("SQL Server");
    expect(config.url_params).toBe("applicationName=dbx&encrypt=false");
  });

  it("selects the independently packaged SQL Server 2008 driver", () => {
    const config = connectionConfig("applicationName=dbx&encrypt=false");

    setSqlServerDriverModeConfig(config, "sqlserver2008");

    expect(sqlServerUses2008Driver(config)).toBe(true);
    expect(sqlServerUsesLegacyCompatibility(config)).toBe(false);
    expect(config.driver_profile).toBe("sqlserver-2008");
    expect(config.driver_label).toBe("SQL Server 2008/2008 R2 Legacy Driver");
    expect(requiredSqlServerCompatibilityDriverKey(config)).toBe("sqlserver-2008");
    expect(config.url_params).toBe("applicationName=dbx&encrypt=false");
  });

  it("recommends TLS 1.0 only for transport-only native failures", () => {
    expect(isSqlServerTlsHandshakeFailure("SQL Server connection failed: TLS handshake EOF")).toBe(true);
    expect(isSqlServerTlsHandshakeFailure("SQL Server TLS handshake EOF\nLogin failed for user (18456)")).toBe(false);
  });

  it("recognizes SSL-unsupported legacy driver failures", () => {
    expect(isSqlServerLegacyTlsUnsupportedFailure("The server is not configured to support SSL")).toBe(true);
    expect(isSqlServerLegacyTlsUnsupportedFailure("\u670d\u52a1\u5668\u672a\u914d\u7f6e\u4e3a\u652f\u6301 SSL")).toBe(true);
    expect(isSqlServerLegacyTlsUnsupportedFailure("TLS handshake EOF")).toBe(false);
  });
});

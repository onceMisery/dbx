import type { ConnectionConfig } from "@/types/database";

export const SQLSERVER_LEGACY_COMPATIBILITY_DRIVER_KEY = "sqlserver-legacy";
export const SQLSERVER_LEGACY_COMPATIBILITY_DRIVER_LABEL = "SQL Server TLS 1.0 compatibility component";
export const SQLSERVER_NATIVE_DRIVER_PROFILE = "sqlserver";
export const SQLSERVER_NATIVE_DRIVER_LABEL = "SQL Server";

const SQLSERVER_ENCRYPTION_DISABLED_VALUES = new Set(["disabled", "disable", "false", "0", "off", "no"]);

export function isSqlServerNativeEncryptionDisabled(params: string | undefined): boolean {
  const normalized = (params || "").trim().replace(/^\?/, "").replace(/;/g, "&");
  if (!normalized) return false;
  const parsed = new URLSearchParams(normalized);
  for (const [key, value] of parsed.entries()) {
    const normalizedKey = key.trim().toLowerCase();
    if (normalizedKey === "sqlserverencryption" || normalizedKey === "encrypt") {
      if (SQLSERVER_ENCRYPTION_DISABLED_VALUES.has(value.trim().toLowerCase())) return true;
    }
  }
  return false;
}

export function setSqlServerNativeEncryptionDisabled(params: string | undefined, disabled: boolean): string {
  const normalized = (params || "").trim().replace(/^\?/, "").replace(/;/g, "&");
  const parsed = new URLSearchParams(normalized);
  for (const key of Array.from(parsed.keys())) {
    const normalizedKey = key.trim().toLowerCase();
    if (normalizedKey === "sqlserverencryption" || normalizedKey === "encrypt") parsed.delete(key);
  }
  if (disabled) parsed.set("sqlserverEncryption", "disabled");
  return parsed.toString();
}

export function sqlServerUsesLegacyCompatibility(config: Pick<ConnectionConfig, "db_type" | "driver_profile">): boolean {
  return config.db_type === "sqlserver" && config.driver_profile === SQLSERVER_LEGACY_COMPATIBILITY_DRIVER_KEY;
}

export function setSqlServerLegacyCompatibilityConfig(config: Pick<ConnectionConfig, "driver_label" | "driver_profile">, enabled: boolean): void {
  config.driver_profile = enabled ? SQLSERVER_LEGACY_COMPATIBILITY_DRIVER_KEY : SQLSERVER_NATIVE_DRIVER_PROFILE;
  config.driver_label = enabled ? SQLSERVER_LEGACY_COMPATIBILITY_DRIVER_LABEL : SQLSERVER_NATIVE_DRIVER_LABEL;
}

export function migrateSqlServerLegacyCompatibilityConfig(_config: Pick<ConnectionConfig, "db_type" | "driver_label" | "driver_profile" | "url_params">): void {
  // Historical disabled-encryption parameters are native transport policy, not a TLS 1.0 driver selection.
}

export function isSqlServerTlsHandshakeFailure(message: string): boolean {
  const text = message.toLowerCase();
  const reachedAuthentication = text.includes("18456") || text.includes("login failed") || text.includes("authentication failed") || text.includes("\u767b\u5f55\u5931\u8d25") || text.includes("\u8eab\u4efd\u9a8c\u8bc1");
  return !reachedAuthentication && text.includes("sql server") && text.includes("tls") && (text.includes("handshake") || text.includes("eof") || text.includes("performing i/o"));
}

export function isSqlServerLegacyTlsUnsupportedFailure(message: string): boolean {
  const text = message.toLowerCase();
  return text.includes("server is not configured to support ssl") || text.includes("server does not support ssl") || text.includes("server does not support encryption") || text.includes("\u670d\u52a1\u5668\u672a\u914d\u7f6e\u4e3a\u652f\u6301 ssl");
}

export function requiresSqlServerLegacyCompatibilityComponent(config: ConnectionConfig): boolean {
  return sqlServerUsesLegacyCompatibility(config);
}

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

function componentSource(name: string): string {
  return readFileSync(new URL(`../../components/mq/${name}`, import.meta.url), "utf8");
}

function connectionComponentSource(name: string): string {
  return readFileSync(new URL(`../../components/connection/${name}`, import.meta.url), "utf8");
}

function libSource(name: string): string {
  return readFileSync(new URL(`../${name}`, import.meta.url), "utf8");
}

describe("MQ list panels", () => {
  it("does not issue per-tenant detail requests from the tenant list UI", () => {
    const source = componentSource("TenantsPanel.vue");

    expect(source).not.toContain("mqGetTenant");
  });

  it("does not issue per-namespace permission requests from the namespace list UI", () => {
    const source = componentSource("NamespacesPanel.vue");

    expect(source).not.toContain("mqListPermissions");
  });

  it("refreshing policies hydrates missing fields from defaults instead of dirty form values", () => {
    const source = componentSource("PoliciesPanel.vue");

    expect(source).not.toContain("policyFormsFromEffectivePolicies(loaded, currentPolicyForms())");
    expect(source).toContain("policyFormsFromEffectivePolicies(loaded, defaultMqPolicyForms())");
  });

  it("raw api panel includes common endpoint presets that use the selected mq context", () => {
    const rawSource = componentSource("RawApiPanel.vue");
    const consoleSource = componentSource("MqAdminConsole.vue");

    expect(rawSource).toContain("const presets = computed<RawApiPreset[]>");
    expect(rawSource).toContain("/admin/v2/brokers/version");
    expect(rawSource).toContain("/internalStats");
    expect(rawSource).toContain("/partitioned-stats");
    expect(rawSource).toContain("/schema");
    expect(rawSource).toContain("presetsCollapsed");
    expect(rawSource).toContain("formatJsonBody");
    expect(rawSource).toContain("bodyTextareaRows");
    expect(rawSource).toContain("body: isReadMethod.value ? undefined : parseBody()");
    expect(consoleSource).toContain(':tenant="selectedTenant"');
    expect(consoleSource).toContain(':namespace="selectedNamespace"');
    expect(consoleSource).toContain(':topic="selectedTopic"');
  });

  it("passes a synthetic topic context for kafka clusters without tenant namespaces", () => {
    const consoleSource = componentSource("MqAdminConsole.vue");

    expect(consoleSource).toContain("isKafkaCluster");
    expect(consoleSource).toContain("effectiveTenant");
    expect(consoleSource).toContain("effectiveNamespace");
    expect(consoleSource).toContain(':tenant="effectiveTenant"');
    expect(consoleSource).toContain(':namespace="effectiveNamespace"');
  });

  it("connection dialog can persist kafka mq bootstrap server configuration", () => {
    const source = connectionComponentSource("ConnectionDialog.vue");

    expect(source).toContain("mqSystemKind.value");
    expect(source).toContain("systemKind: mqSystemKind.value");
    expect(source).toContain("mqKafkaBootstrapServers");
    expect(source).toContain("bootstrapServers");
    expect(source).toContain("Apache Kafka");
    expect(source).toContain('adminUrl: ""');
    expect(source).toContain("normalizeMqKafkaBootstrapServers");
    expect(source).toContain("without a URL scheme");
    expect(source).toContain("MQ_KAFKA_SECURITY_PROTOCOL_AUTO");
    expect(source).not.toContain('{ value: "", label: "Auto" }');
    expect(source).toContain('config.driver_profile = mqConfig.systemKind === "kafka" ? "kafka" : "pulsar"');
    expect(source).toContain('config.driver_label = mqConfig.systemKind === "kafka" ? "Apache Kafka" : "Apache Pulsar"');
  });

  it("mq api wrappers expose send message on tauri web and lazy api layers", () => {
    expect(libSource("mq-tauri.ts")).toContain("mqSendMessage");
    expect(libSource("mq-http.ts")).toContain("mqSendMessage");
    expect(libSource("api.ts")).toContain('export const mqSendMessage = forward("mqSendMessage")');
  });
});

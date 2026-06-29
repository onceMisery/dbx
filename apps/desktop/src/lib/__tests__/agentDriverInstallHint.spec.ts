import { describe, expect, it } from "vitest";
import { showAgentDriverInstallHint, type AgentDriverInstallState } from "../agentDriverInstallHint";

const drivers = (installed: boolean): AgentDriverInstallState[] => [{ db_type: "kafka", installed }];

describe("agentDriverInstallHint", () => {
  it("shows the agent driver install hint for kafka mq profiles only when kafka is missing", () => {
    expect(showAgentDriverInstallHint("mq", drivers(false), "kafka")).toBe(true);
    expect(showAgentDriverInstallHint("mq", drivers(true), "kafka")).toBe(false);
  });

  it("does not ask pulsar mq profiles to install a kafka agent", () => {
    expect(showAgentDriverInstallHint("mq", drivers(false), "pulsar")).toBe(false);
    expect(showAgentDriverInstallHint("mq", drivers(false), "mq")).toBe(false);
  });
});

import { describe, expect, it } from "vitest";
import { buildMqKafkaExtra, mqKafkaConnectionTarget, normalizeMqKafkaBootstrapServers, normalizeMqKafkaZooKeeperServers, resolveMqKafkaConnectionSource } from "../mqKafkaConnection";

describe("mqKafkaConnection", () => {
  it("keeps existing bootstrap server configurations on the bootstrap source", () => {
    expect(resolveMqKafkaConnectionSource({ bootstrapServers: "broker-1:9092" })).toBe("bootstrap");
  });

  it("recognizes explicit and legacy ZooKeeper discovery configurations", () => {
    expect(resolveMqKafkaConnectionSource({ connectionSource: "zookeeper", zookeeperServers: "zk-1:2181" })).toBe("zookeeper");
    expect(resolveMqKafkaConnectionSource({ zookeeperServers: "zk-legacy:2181" })).toBe("zookeeper");
  });

  it("normalizes Kafka bootstrap and ZooKeeper server lists", () => {
    expect(normalizeMqKafkaBootstrapServers(" broker-1:9092, broker-2:9093 ")).toBe("broker-1:9092,broker-2:9093");
    expect(normalizeMqKafkaZooKeeperServers(" zk-1:2181; zk-2:2181/kafka ")).toBe("zk-1:2181,zk-2:2181/kafka");
  });

  it("builds source-specific extra fields without a fake bootstrap address", () => {
    expect(
      buildMqKafkaExtra({
        connectionSource: "zookeeper",
        bootstrapServers: "ignored:9092",
        zookeeperServers: "zk-1:2181, zk-2:2181/kafka",
        securityProtocol: "SASL_SSL",
        saslMechanism: "SCRAM-SHA-256",
      }),
    ).toEqual({
      connectionSource: "zookeeper",
      zookeeperServers: "zk-1:2181,zk-2:2181/kafka",
      securityProtocol: "SASL_SSL",
      saslMechanism: "SCRAM-SHA-256",
    });

    expect(
      buildMqKafkaExtra({
        connectionSource: "bootstrap",
        bootstrapServers: "broker-1:9092",
        zookeeperServers: "ignored:2181",
        securityProtocol: "",
        saslMechanism: "",
      }),
    ).toEqual({ bootstrapServers: "broker-1:9092" });
  });

  it("maps the active source to the generic connection target", () => {
    expect(
      mqKafkaConnectionTarget({
        connectionSource: "bootstrap",
        bootstrapServers: "broker-1:9093,broker-2:9093",
        zookeeperServers: "",
        securityProtocol: "SSL",
      }),
    ).toEqual({ host: "broker-1", port: 9093, ssl: true });

    expect(
      mqKafkaConnectionTarget({
        connectionSource: "zookeeper",
        bootstrapServers: "",
        zookeeperServers: "zk-1:2281,zk-2:2181/kafka",
        securityProtocol: "SASL_SSL",
      }),
    ).toEqual({ host: "zk-1", port: 2281, ssl: false });
  });
});

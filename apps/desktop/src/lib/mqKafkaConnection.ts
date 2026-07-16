import { firstZooKeeperEndpoint, normalizeZooKeeperConnectString } from "@/lib/zookeeperConnection";

export type MqKafkaConnectionSource = "bootstrap" | "zookeeper";

export interface MqKafkaConnectionExtraInput {
  connectionSource: MqKafkaConnectionSource;
  bootstrapServers: string;
  zookeeperServers: string;
  securityProtocol?: string;
  saslMechanism?: string;
}

export interface MqKafkaConnectionTarget {
  host: string;
  port: number;
  ssl: boolean;
}

export function resolveMqKafkaConnectionSource(extra: Record<string, unknown>): MqKafkaConnectionSource {
  if (extra.connectionSource === "zookeeper") return "zookeeper";
  if (typeof extra.zookeeperServers === "string" && extra.zookeeperServers.trim() && !(typeof extra.bootstrapServers === "string" && extra.bootstrapServers.trim())) {
    return "zookeeper";
  }
  return "bootstrap";
}

function requireValue(value: string, message: string): string {
  const trimmed = value.trim();
  if (!trimmed) throw new Error(message);
  return trimmed;
}

function normalizeMqKafkaBootstrapServer(server: string): string {
  if (server.includes("://")) {
    throw new Error("Kafka bootstrap servers must be host:port values without a URL scheme");
  }
  let parsed: URL;
  try {
    parsed = new URL(`kafka://${server}`);
  } catch {
    throw new Error("Kafka bootstrap servers are invalid");
  }
  if (!parsed.hostname || parsed.username || parsed.password || parsed.search || parsed.hash || (parsed.pathname && parsed.pathname !== "/")) {
    throw new Error("Kafka bootstrap servers are invalid");
  }
  return server;
}

export function normalizeMqKafkaBootstrapServers(value: string): string {
  const servers = requireValue(value, "Kafka bootstrap servers are required")
    .split(",")
    .map((server) => server.trim())
    .filter(Boolean)
    .map(normalizeMqKafkaBootstrapServer);
  if (!servers.length) throw new Error("Kafka bootstrap servers are required");
  return servers.join(",");
}

export function normalizeMqKafkaZooKeeperServers(value: string): string {
  const servers = normalizeZooKeeperConnectString(requireValue(value, "Kafka ZooKeeper servers are required"));
  for (const server of servers.split(",")) {
    const endpoint = firstZooKeeperEndpoint(server);
    if (!endpoint?.host || endpoint.port < 1 || endpoint.port > 65535) {
      throw new Error("Kafka ZooKeeper servers are invalid");
    }
  }
  return servers;
}

export function buildMqKafkaExtra(input: MqKafkaConnectionExtraInput): Record<string, string> {
  const extra: Record<string, string> =
    input.connectionSource === "zookeeper"
      ? {
          connectionSource: "zookeeper",
          zookeeperServers: normalizeMqKafkaZooKeeperServers(input.zookeeperServers),
        }
      : { bootstrapServers: normalizeMqKafkaBootstrapServers(input.bootstrapServers) };

  const securityProtocol = input.securityProtocol?.trim();
  const saslMechanism = input.saslMechanism?.trim();
  if (securityProtocol) extra.securityProtocol = securityProtocol;
  if (saslMechanism) extra.saslMechanism = saslMechanism;
  return extra;
}

export function mqKafkaConnectionTarget(input: MqKafkaConnectionExtraInput): MqKafkaConnectionTarget {
  if (input.connectionSource === "zookeeper") {
    const endpoint = firstZooKeeperEndpoint(normalizeMqKafkaZooKeeperServers(input.zookeeperServers));
    if (!endpoint) throw new Error("Kafka ZooKeeper servers are required");
    return { ...endpoint, ssl: false };
  }

  const first = normalizeMqKafkaBootstrapServers(input.bootstrapServers).split(",")[0];
  let parsed: URL;
  try {
    parsed = new URL(`kafka://${first}`);
  } catch {
    throw new Error("Kafka bootstrap servers are invalid");
  }
  return {
    host: parsed.hostname,
    port: Number(parsed.port) || 9092,
    ssl: input.securityProtocol === "SSL" || input.securityProtocol === "SASL_SSL",
  };
}

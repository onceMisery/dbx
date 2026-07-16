package com.dbx.agent.kafka;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.apache.kafka.clients.admin.AlterConfigOp;
import org.apache.kafka.clients.admin.Config;
import org.apache.kafka.clients.admin.ConfigEntry;
import org.apache.zookeeper.CreateMode;
import org.apache.zookeeper.Watcher;
import org.apache.zookeeper.ZooDefs;
import org.apache.zookeeper.ZooKeeper;
import org.apache.zookeeper.server.NIOServerCnxnFactory;
import org.apache.zookeeper.server.ZooKeeperServer;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

final class KafkaAgentTest {

    @TempDir
    Path tempDir;

    @Test
    void resolvesBootstrapServersFromKafka11ZooKeeperRegistrationWithChroot() throws Exception {
        Path snapshots = Files.createDirectory(tempDir.resolve("snapshots"));
        Path logs = Files.createDirectory(tempDir.resolve("logs"));
        ZooKeeperServer server = new ZooKeeperServer(snapshots.toFile(), logs.toFile(), 2_000);
        NIOServerCnxnFactory factory = new NIOServerCnxnFactory();
        factory.configure(new InetSocketAddress("127.0.0.1", 0), 10);
        factory.startup(server);

        ZooKeeper client = null;
        try {
            CountDownLatch connected = new CountDownLatch(1);
            System.setProperty("zookeeper.sasl.client", "false");
            client = new ZooKeeper("127.0.0.1:" + factory.getLocalPort(), 5_000, event -> {
                if (event.getState() == Watcher.Event.KeeperState.SyncConnected) connected.countDown();
            });
            Assertions.assertTrue(connected.await(5, TimeUnit.SECONDS));
            client.create("/kafka", new byte[0], ZooDefs.Ids.OPEN_ACL_UNSAFE, CreateMode.PERSISTENT);
            client.create("/kafka/brokers", new byte[0], ZooDefs.Ids.OPEN_ACL_UNSAFE, CreateMode.PERSISTENT);
            client.create("/kafka/brokers/ids", new byte[0], ZooDefs.Ids.OPEN_ACL_UNSAFE, CreateMode.PERSISTENT);
            client.create(
                "/kafka/brokers/ids/0",
                ("{\"listener_security_protocol_map\":{\"PLAINTEXT\":\"PLAINTEXT\"},"
                    + "\"endpoints\":[\"PLAINTEXT://legacy-broker:9092\"]}").getBytes(StandardCharsets.UTF_8),
                ZooDefs.Ids.OPEN_ACL_UNSAFE,
                CreateMode.EPHEMERAL
            );
            client.create(
                "/kafka/brokers/ids/1",
                "[]".getBytes(StandardCharsets.UTF_8),
                ZooDefs.Ids.OPEN_ACL_UNSAFE,
                CreateMode.EPHEMERAL
            );

            JsonObject connection = new JsonObject();
            connection.addProperty(
                "zookeeper_connect_string",
                "127.0.0.1:" + factory.getLocalPort() + "/kafka"
            );
            connection.addProperty("security_protocol", "PLAINTEXT");
            connection.addProperty("zookeeper_connection_timeout_ms", 5_000);

            JsonObject resolved = KafkaAgent.resolveBrokerConnection(connection);

            Assertions.assertEquals("legacy-broker:9092", resolved.get("bootstrap_servers").getAsString());
        } finally {
            if (client != null) client.close();
            factory.shutdown();
            server.shutdown();
            server.getTxnLogFactory().close();
        }
    }

    @Test
    void brokerEndpointsSelectMatchingProtocolAndKeepBrokerOrder() {
        List<JsonObject> registrations = Arrays.asList(
            broker("{\"listener_security_protocol_map\":{\"INTERNAL\":\"PLAINTEXT\",\"EXTERNAL\":\"SSL\"},"
                + "\"endpoints\":[\"INTERNAL://broker-2:9092\",\"EXTERNAL://public-2:9093\"]}"),
            broker("{\"listener_security_protocol_map\":{\"INTERNAL\":\"PLAINTEXT\",\"EXTERNAL\":\"SSL\"},"
                + "\"endpoints\":[\"EXTERNAL://public-1:9093\",\"INTERNAL://broker-1:9092\"]}")
        );

        Assertions.assertEquals("public-2:9093,public-1:9093", KafkaAgent.brokerEndpoints(registrations, "SSL"));
    }

    @Test
    void brokerEndpointsFallBackToLegacyHostAndPort() {
        List<JsonObject> registrations = Collections.singletonList(
            broker("{\"host\":\"legacy-broker\",\"port\":9092}")
        );

        Assertions.assertEquals("legacy-broker:9092", KafkaAgent.brokerEndpoints(registrations, "PLAINTEXT"));
    }

    @Test
    void brokerEndpointsSkipMalformedRegistrationWhenAnotherBrokerIsUsable() {
        List<JsonObject> registrations = Arrays.asList(
            broker("{\"host\":\"broken\",\"port\":\"not-a-port\"}"),
            broker("{\"host\":\"healthy-broker\",\"port\":9092}")
        );

        Assertions.assertEquals("healthy-broker:9092", KafkaAgent.brokerEndpoints(registrations, "PLAINTEXT"));
    }

    @Test
    void brokerEndpointsRejectRegistrationsWithoutUsableAddresses() {
        IllegalArgumentException error = Assertions.assertThrows(
            IllegalArgumentException.class,
            () -> KafkaAgent.brokerEndpoints(Collections.singletonList(broker("{\"endpoints\":[]}")), "PLAINTEXT")
        );

        Assertions.assertTrue(error.getMessage().contains("usable Kafka broker endpoints"));
    }

    @Test
    void brokerEndpointsDoNotUseLegacyAddressWhenRegisteredListenersHaveAnotherProtocol() {
        JsonObject registration = broker(
            "{\"listener_security_protocol_map\":{\"INTERNAL\":\"PLAINTEXT\"},"
                + "\"endpoints\":[\"INTERNAL://broker-1:9092\"],\"host\":\"broker-1\",\"port\":9092}"
        );

        Assertions.assertThrows(
            IllegalArgumentException.class,
            () -> KafkaAgent.brokerEndpoints(Collections.singletonList(registration), "SSL")
        );
    }

    @Test
    void legacyTopicConfigAppliesSetAndDeleteWithoutLosingExistingOverrides() {
        Config current = new Config(Arrays.asList(
            new ConfigEntry("cleanup.policy", "delete"),
            new ConfigEntry("retention.ms", "60000")
        ));
        List<AlterConfigOp> ops = Arrays.asList(
            new AlterConfigOp(new ConfigEntry("retention.ms", "120000"), AlterConfigOp.OpType.SET),
            new AlterConfigOp(new ConfigEntry("cleanup.policy", null), AlterConfigOp.OpType.DELETE)
        );

        Map<String, String> merged = KafkaAgent.legacyTopicConfig(current, ops);

        Assertions.assertEquals(Collections.singletonMap("retention.ms", "120000"), merged);
    }

    @Test
    void legacyTopicConfigRejectsAppendAndSubtractOperations() {
        Config current = new Config(Collections.singletonList(new ConfigEntry("cleanup.policy", "delete")));
        AlterConfigOp append = new AlterConfigOp(
            new ConfigEntry("cleanup.policy", "compact"),
            AlterConfigOp.OpType.APPEND
        );

        IllegalArgumentException error = Assertions.assertThrows(
            IllegalArgumentException.class,
            () -> KafkaAgent.legacyTopicConfig(current, Collections.singletonList(append))
        );

        Assertions.assertTrue(error.getMessage().contains("APPEND"));
    }

    private static JsonObject broker(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }
}

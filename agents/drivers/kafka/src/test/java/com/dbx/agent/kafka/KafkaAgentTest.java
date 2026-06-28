package com.dbx.agent.kafka;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.apache.kafka.clients.admin.OffsetSpec;
import org.apache.kafka.common.acl.AccessControlEntry;
import org.apache.kafka.common.acl.AclBinding;
import org.apache.kafka.common.acl.AclOperation;
import org.apache.kafka.common.acl.AclPermissionType;
import org.apache.kafka.common.resource.PatternType;
import org.apache.kafka.common.resource.ResourcePattern;
import org.apache.kafka.common.resource.ResourceType;
import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Properties;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class KafkaAgentTest {

    @Test
    void offsetSpecForPositionSupportsTimestamp() {
        OffsetSpec spec = KafkaAgent.offsetSpecForPosition("timestamp", 1_710_000_000_000L);

        assertTrue(spec.getClass().getName().contains("Timestamp"));
    }

    @Test
    void offsetSpecForPositionRejectsTimestampWithoutMillis() {
        assertThrows(IllegalArgumentException.class, () -> KafkaAgent.offsetSpecForPosition("timestamp", null));
    }

    @Test
    void securityPropertiesAcceptTopLevelTlsSkipVerify() {
        JsonObject conn = JsonParser.parseString(
            """
            {
              "security_protocol": "SASL_SSL",
              "sasl_mechanism": "PLAIN",
              "sasl_username": "alice",
              "sasl_password": "secret",
              "tls_skip_verify": true
            }
            """
        ).getAsJsonObject();
        Properties props = new Properties();

        KafkaAgent.applySecurityProperties(conn, props);

        assertEquals("SASL_SSL", props.getProperty("security.protocol"));
        assertEquals("PLAIN", props.getProperty("sasl.mechanism"));
        assertEquals("", props.getProperty("ssl.endpoint.identification.algorithm"));
    }

    @Test
    void securityPropertiesEscapeJaasCredentialValues() {
        JsonObject conn = JsonParser.parseString(
            """
            {
              "security_protocol": "SASL_PLAINTEXT",
              "sasl_mechanism": "PLAIN",
              "sasl_username": "ali\\\\ce",
              "sasl_password": "p\\\\\\\"w"
            }
            """
        ).getAsJsonObject();
        Properties props = new Properties();

        KafkaAgent.applySecurityProperties(conn, props);

        assertEquals(
            "org.apache.kafka.common.security.plain.PlainLoginModule required username=\"ali\\\\ce\" password=\"p\\\\\\\"w\";",
            props.getProperty("sasl.jaas.config")
        );
    }

    @Test
    void connectionPropertiesIncludeExtraClientProperties() {
        JsonObject conn = JsonParser.parseString(
            """
            {
              "security_protocol": "PLAINTEXT",
              "properties": {
                "client.id": "dbx-kafka-test",
                "metadata.max.age.ms": 5000
              }
            }
            """
        ).getAsJsonObject();
        Properties props = new Properties();

        KafkaAgent.applyConnectionProperties(conn, props);

        assertEquals("PLAINTEXT", props.getProperty("security.protocol"));
        assertEquals("dbx-kafka-test", props.getProperty("client.id"));
        assertEquals("5000", props.getProperty("metadata.max.age.ms"));
    }

    @Test
    void shutdownRequestReturnsJsonResponseBeforeProcessExit() {
        JsonObject response = JsonParser.parseString(
            KafkaAgent.handleRequest("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"shutdown\",\"params\":{}}")
        ).getAsJsonObject();

        assertEquals(1, response.get("id").getAsInt());
        assertTrue(response.getAsJsonObject("result").get("ok").getAsBoolean());
    }

    @Test
    void deletedAclCountUsesDeletedBindingsNotFilterCount() {
        AclBinding read = aclBinding("events", AclOperation.READ);
        AclBinding write = aclBinding("events", AclOperation.WRITE);

        assertEquals(2, KafkaAgent.deletedAclCount(List.of(read, write)));
    }

    private static AclBinding aclBinding(String resourceName, AclOperation operation) {
        ResourcePattern pattern = new ResourcePattern(ResourceType.TOPIC, resourceName, PatternType.LITERAL);
        AccessControlEntry entry = new AccessControlEntry("User:alice", "*", operation, AclPermissionType.ALLOW);
        return new AclBinding(pattern, entry);
    }
}

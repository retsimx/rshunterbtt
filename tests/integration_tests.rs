use rshunterbtt::App;
use rshunterbtt::config::Config;
use rshunterbtt::traits::{MockBleClient, MockMqttClient, MockDatabaseWriter};
use rshunterbtt::protocol::{Second83Protocol};
use std::sync::Arc;
use mockall::predicate::*;
use mockall::predicate;

fn mock_config() -> Config {
    Config {
        device_address: "11:22:33:44:55:66".to_string(),
        device_name: "test_device".to_string(),
        mqtt_broker: "localhost".to_string(),
        mqtt_port: 1883,
        mqtt_sub_topic: "sub".to_string(),
        mqtt_pub_topic: "pub".to_string(),
        influxdb_url: "http://localhost:8086".to_string(),
        influxdb_token: "token".to_string(),
        influxdb_org: "org".to_string(),
        influxdb_bucket: "bucket".to_string(),
    }
}

#[tokio::test]
async fn test_full_irrigation_flow() {
    // 1. Setup mocks
    let mut ble = MockBleClient::new();
    let mut mqtt = MockMqttClient::new();
    let db = MockDatabaseWriter::new();

    // 2. Expect MQTT message will trigger BLE actions
    ble.expect_is_connected().returning(|| Box::pin(async { true }));
    ble.expect_read_protocol_83().returning(|| Box::pin(async { Ok(Second83Protocol::default()) }));
    ble.expect_read_status().returning(|| Box::pin(async { Ok(vec![0; 20]) }));
    
    // Starting zone 1
    ble.expect_write_protocol_86().returning(|_| Box::pin(async { Ok(()) }));
    ble.expect_write_protocol_83().returning(|_| Box::pin(async { Ok(()) }));

    // 3. Expect MQTT publish response
    mqtt.expect_publish()
        .with(eq("pub"), predicate::str::contains(r#""success":true"#))
        .returning(|_, _| Box::pin(async { Ok(()) }));

    let app = App::new(
        mock_config(),
        Arc::new(ble),
        Arc::new(mqtt),
        Arc::new(db),
    );

    // 4. Simulate receiving MQTT message
    let payload = r#"{"cmd":"on_off","zone":"flower","on_off":true}"#;
    app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
}

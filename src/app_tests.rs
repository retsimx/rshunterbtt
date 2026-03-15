#[cfg(test)]
mod tests {
    use crate::App;
    use crate::traits::{MockBleClient, MockMqttClient, MockDatabaseWriter};
    use crate::config::Config;
    use crate::protocol::Second83Protocol;
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
    async fn test_run_command_start_zone1() {
        let mut ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected().returning(|| Box::pin(async { true }));
        ble.expect_read_protocol_83().returning(|| Box::pin(async { Ok(Second83Protocol::default()) }));
        ble.expect_read_status().returning(|| Box::pin(async { Ok(vec![0; 20]) }));
        
        ble.expect_write_protocol_86()
            .withf(|p| p.zm_hour == 2 && p.w_index == 4)
            .returning(|_| Box::pin(async { Ok(()) }));
            
        ble.expect_write_protocol_83()
            .withf(|p| p.zone1_enable_manual == 1)
            .returning(|_| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));
        let result = app.run_command("start", 1, 7200).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_run_command_stop_zone2() {
        let mut ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected().returning(|| Box::pin(async { true }));
        ble.expect_read_protocol_83().returning(|| Box::pin(async { 
            let mut p = Second83Protocol::default();
            p.zone2_enable_manual = 1;
            Ok(p)
        }));
        ble.expect_read_status().returning(|| Box::pin(async { Ok(vec![0; 20]) }));
            
        ble.expect_write_protocol_83()
            .withf(|p| p.zone2_enable_manual == 0)
            .returning(|_| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));
        let result = app.run_command("stop", 2, 0).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_get_status_active() {
        let mut ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected().returning(|| Box::pin(async { true }));
        ble.expect_read_status().returning(|| Box::pin(async { 
            let mut data = vec![0; 20];
            data[8] = 1; // Zone 2 active
            Ok(data)
        }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));
        assert!(app.get_status(2).await.unwrap());
        assert!(!app.get_status(1).await.unwrap());
    }

    #[tokio::test]
    async fn test_handle_mqtt_status_request() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected().returning(|| Box::pin(async { true }));
        ble.expect_read_status().returning(|| Box::pin(async { 
            let mut data = vec![0; 20];
            data[4] = 1; // Zone 1 active
            Ok(data)
        }));

        mqtt.expect_publish()
            .with(eq("pub"), predicate::str::contains(r#""status":1"#))
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));
        let payload = r#"{"cmd":"status","zone":"flower"}"#; // flower is zone 1
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_handle_mqtt_invalid_json() {
        let ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();
        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));
        
        let result = app.handle_mqtt_message(b"invalid").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_mqtt_unknown_command() {
        let ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();
        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));
        
        let payload = r#"{"cmd":"unknown","zone":"grass"}"#;
        let result = app.handle_mqtt_message(payload.as_bytes()).await;
        assert!(result.is_ok()); // Logic just logs error and returns Ok(())
    }

    #[tokio::test]
    async fn test_poll_battery() {
        let mut ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let mut db = MockDatabaseWriter::new();

        ble.expect_is_connected().returning(|| Box::pin(async { true }));
        ble.expect_read_battery().returning(|| Box::pin(async { Ok(85) }));
        
        db.expect_write_battery()
            .with(eq("test_device"), eq(85))
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(
            mock_config(),
            Arc::new(ble),
            Arc::new(mqtt),
            Arc::new(db),
        );

        app.poll_battery().await.unwrap();
    }

    #[tokio::test]
    async fn test_poll_battery_failure() {
        let mut ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        // Simulate connection failure
        ble.expect_is_connected().returning(|| Box::pin(async { false }));
        ble.expect_connect()
            .with(eq("11:22:33:44:55:66"))
            .returning(|_| Box::pin(async { Err(anyhow::anyhow!("Connection timed out")) }));

        let app = App::new(
            mock_config(),
            Arc::new(ble),
            Arc::new(mqtt),
            Arc::new(db),
        );

        let result = app.poll_battery().await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Connection timed out");
    }
}

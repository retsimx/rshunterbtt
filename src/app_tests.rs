#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::protocol::Second83Protocol;
    use crate::traits::{MockBleClient, MockDatabaseWriter, MockMqttClient};
    use crate::App;
    use crate::{
        next_backoff, run_battery_polling_loop, run_connection_supervisor, BatteryPollingGuard,
        BatteryPollingIntervals, INITIAL_BACKOFF,
    };
    use mockall::predicate;
    use mockall::predicate::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::watch;

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
            device_password: None,
        }
    }

    #[tokio::test]
    async fn test_run_command_start_zone1() {
        let mut ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_protocol_83()
            .returning(|| Box::pin(async { Ok(Second83Protocol::default()) }));
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));

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

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_protocol_83().returning(|| {
            Box::pin(async {
                let mut p = Second83Protocol::default();
                p.zone2_enable_manual = 1;
                Ok(p)
            })
        });
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));

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

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_status().returning(|| {
            Box::pin(async {
                let mut data = vec![0; 20];
                data[8] = 1; // Zone 2 active
                Ok(data)
            })
        });

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));
        assert!(app.get_status(2).await.unwrap());
        assert!(!app.get_status(1).await.unwrap());
    }

    #[tokio::test]
    async fn test_handle_mqtt_status_request() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_status().returning(|| {
            Box::pin(async {
                let mut data = vec![0; 20];
                data[4] = 1; // Zone 1 active
                Ok(data)
            })
        });

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

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_battery()
            .returning(|| Box::pin(async { Ok(85) }));

        db.expect_write_battery()
            .with(eq("test_device"), eq(85))
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));

        app.poll_battery().await.unwrap();
    }

    #[tokio::test]
    async fn test_poll_battery_failure_when_connection_not_ready() {
        tokio::time::pause();
        let ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        let (ready_tx, ready_rx) = watch::channel(false);
        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db))
            .with_connection_ready(ready_rx);

        // The connection never becomes ready; ensure_connection_ready must wait
        // through intermediate false updates and then time out.
        let ready_tx2 = ready_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let _ = ready_tx2.send(false);
        });

        let fut = app.poll_battery();
        tokio::pin!(fut);
        tokio::time::advance(Duration::from_secs(70)).await;
        let result = fut.await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ensure_connection_ready_waits_for_recovery() {
        let ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        let (ready_tx, ready_rx) = watch::channel(false);
        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db))
            .with_connection_ready(ready_rx);

        // Send an intermediate false, then true. The gate must not return on the
        // false; it must wait until the connection becomes ready.
        let ready_tx2 = ready_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let _ = ready_tx2.send(false);
            tokio::time::sleep(Duration::from_millis(10)).await;
            let _ = ready_tx2.send(true);
        });

        let result = app.ensure_connection_ready().await;
        assert!(result.is_ok());
    }

    fn test_battery_polling_intervals() -> BatteryPollingIntervals {
        BatteryPollingIntervals {
            success: Duration::from_millis(50),
            failure: Duration::from_millis(10),
        }
    }

    fn mock_app_with_battery_counter(poll_count: Arc<AtomicUsize>) -> Arc<App> {
        let mut ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let mut db = MockDatabaseWriter::new();

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));

        let count_for_read = poll_count.clone();
        ble.expect_read_battery().returning(move || {
            let count = count_for_read.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(85)
            })
        });

        db.expect_write_battery()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        Arc::new(App::new(
            mock_config(),
            Arc::new(ble),
            Arc::new(mqtt),
            Arc::new(db),
        ))
    }

    #[tokio::test]
    async fn test_uncancelled_battery_tasks_accumulate_polls() {
        let poll_count = Arc::new(AtomicUsize::new(0));
        let app = mock_app_with_battery_counter(poll_count.clone());
        let intervals = test_battery_polling_intervals();
        let (_stop_tx, stop_rx) = watch::channel(false);

        tokio::spawn(run_battery_polling_loop(
            app.clone(),
            stop_rx.clone(),
            intervals,
        ));
        tokio::spawn(run_battery_polling_loop(app, stop_rx, intervals));

        tokio::time::sleep(Duration::from_millis(120)).await;

        assert!(
            poll_count.load(Ordering::SeqCst) > 2,
            "expected multiple uncancelled loops to accumulate polls, got {}",
            poll_count.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn test_battery_polling_does_not_accumulate_on_instance_restart() {
        let poll_count = Arc::new(AtomicUsize::new(0));
        let app = mock_app_with_battery_counter(poll_count.clone());
        let intervals = test_battery_polling_intervals();

        {
            let _guard = BatteryPollingGuard::start(app.clone(), intervals);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(
            poll_count.load(Ordering::SeqCst),
            1,
            "stopped instance should not keep polling"
        );

        {
            let _guard = BatteryPollingGuard::start(app.clone(), intervals);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(
            poll_count.load(Ordering::SeqCst),
            2,
            "restarted instance should add exactly one new poller"
        );
    }

    #[test]
    fn test_next_backoff_sequence_and_cap() {
        let mut current = INITIAL_BACKOFF;
        let mut delays = Vec::new();
        for _ in 0..10 {
            current = next_backoff(current);
            delays.push(current);
        }
        let expected = vec![
            Duration::from_secs(2),
            Duration::from_secs(4),
            Duration::from_secs(8),
            Duration::from_secs(16),
            Duration::from_secs(32),
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(60),
        ];
        assert_eq!(delays, expected);
    }

    async fn wait_until<F: Fn() -> bool>(cond: F) {
        for _ in 0..250 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("condition not met within timeout");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_connection_supervisor_reconnects_after_disconnect() {
        let mut ble = MockBleClient::new();
        let mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        let connect_count = Arc::new(AtomicUsize::new(0));
        let write_pw_count = Arc::new(AtomicUsize::new(0));
        let subscribe_count = Arc::new(AtomicUsize::new(0));
        let disconnect_pending = Arc::new(AtomicBool::new(false));

        let cc = connect_count.clone();
        ble.expect_connect().returning(move |_| {
            cc.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        });

        let wc = write_pw_count.clone();
        ble.expect_write_password().returning(move |_| {
            wc.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        });

        let sc = subscribe_count.clone();
        ble.expect_subscribe_notifications().returning(move |_| {
            sc.fetch_add(1, Ordering::SeqCst);
            let (_, rx) = tokio::sync::mpsc::channel(1);
            Box::pin(async move { Ok(rx) })
        });

        let dp = disconnect_pending.clone();
        ble.expect_is_connected().returning(move || {
            let d = dp.clone();
            Box::pin(async move { !d.swap(false, Ordering::SeqCst) })
        });

        ble.expect_read_protocol_83()
            .returning(|| Box::pin(async { Ok(Second83Protocol::default()) }));
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));
        ble.expect_write_protocol_86()
            .returning(|_| Box::pin(async { Ok(()) }));
        ble.expect_write_protocol_83()
            .returning(|_| Box::pin(async { Ok(()) }));

        let ble = Arc::new(ble);
        let config = mock_config();

        let (stop_tx, stop_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = watch::channel(false);
        let ready_check = ready_rx.clone();

        let app = Arc::new(
            App::new(config.clone(), ble.clone(), Arc::new(mqtt), Arc::new(db))
                .with_connection_ready(ready_rx),
        );

        let supervisor_ble = ble.clone();
        let supervisor_config = config;
        tokio::spawn(run_connection_supervisor(
            supervisor_ble,
            supervisor_config,
            stop_rx,
            ready_tx,
        ));

        wait_until(|| connect_count.load(Ordering::SeqCst) >= 1 && *ready_check.borrow()).await;
        assert_eq!(connect_count.load(Ordering::SeqCst), 1);
        assert_eq!(write_pw_count.load(Ordering::SeqCst), 1);
        assert_eq!(subscribe_count.load(Ordering::SeqCst), 1);

        for _ in 0..3 {
            app.run_command("start", 1, 7200).await.unwrap();
        }
        assert_eq!(
            connect_count.load(Ordering::SeqCst),
            1,
            "no reconnect should occur while connection is ready"
        );

        disconnect_pending.store(true, Ordering::SeqCst);

        wait_until(|| {
            connect_count.load(Ordering::SeqCst) >= 2
                && write_pw_count.load(Ordering::SeqCst) >= 2
                && subscribe_count.load(Ordering::SeqCst) >= 2
        })
        .await;

        let _ = stop_tx.send(true);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::protocol::{Second82Protocol, Second83Protocol};
    use crate::resilience::{ResilienceLadder, ResilienceStateStore};
    use crate::traits::{
        MockBleClient, MockDatabaseWriter, MockMqttClient, MockResilienceController,
    };
    use crate::App;
    use crate::{
        handle_connection_failure, next_backoff, run_battery_polling_loop,
        run_connection_supervisor, BatteryPollingGuard, BatteryPollingIntervals, StatusCacheSender,
        ZoneNamesCacheSender, INITIAL_BACKOFF,
    };
    use anyhow::anyhow;
    use mockall::predicate;
    use mockall::predicate::*;
    use mockall::PredicateBooleanExt;
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
            default_run_seconds: 7200,
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
                data[11] = 1; // Zone 2 active (byte 11)
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
                data[10] = 1; // Zone 1 active (byte 10)
                data[1] = 1; // suspend_watering
                Ok(data)
            })
        });

        mqtt.expect_publish()
            .with(
                eq("pub"),
                predicate::str::contains(r#""status":1"#)
                    .and(predicate::str::contains(r#""suspend_watering":true"#)),
            )
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));
        let payload = r#"{"cmd":"status","zone":"flower"}"#; // flower is zone 1
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_status_uses_cached_notification_without_reread() {
        let ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        // No read_status expectation is set: if get_status issues a GATT read,
        // mockall will panic on the unexpected call, proving no re-read happens.

        mqtt.expect_publish()
            .with(
                eq("pub"),
                predicate::str::contains(r#""status":1"#)
                    .and(predicate::str::contains(r#""suspend_watering":true"#)),
            )
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let (status_tx, status_rx) = watch::channel(None);
        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db))
            .with_status_cache(status_rx, status_tx.clone());

        // Inject an ff82 notification with a changed zone state directly into the
        // watch cache (as the notification-consumer task would).
        let mut data = vec![0u8; 14];
        data[1] = 1; // suspend_watering (byte 1)
        data[10] = 1; // zone1 active (byte 10)
        let parsed = Second82Protocol::from_bytes(&data).unwrap();
        status_tx.send(Some(parsed)).unwrap();

        let payload = r#"{"cmd":"status","zone":"flower"}"#; // flower is zone 1
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_notification_consumer_updates_status_cache() {
        let ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        // No read_status expectation: the notification, not a GATT read, must
        // serve the status (mockall panics on unexpected calls).
        mqtt.expect_publish()
            .with(
                eq("pub"),
                predicate::str::contains(r#""status":1"#)
                    .and(predicate::str::contains(r#""suspend_watering":true"#)),
            )
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let (status_tx, mut status_rx) = watch::channel(None);
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let (notif_tx, notif_rx) = tokio::sync::mpsc::channel(1);
        crate::spawn_notification_consumer(notif_rx, status_tx.clone(), shutdown_rx);

        let mut data = vec![0u8; 14];
        data[1] = 1; // suspend_watering (byte 1)
        data[10] = 1; // zone1 active (byte 10)
        notif_tx.send(data).await.unwrap();
        status_rx.changed().await.unwrap();

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db))
            .with_status_cache(status_rx, status_tx);

        let payload = r#"{"cmd":"status","zone":"flower"}"#; // flower is zone 1
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_status_seed_read_on_connect() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        // Cache is None right after connect (no notification yet), so exactly ONE
        // seed GATT read must occur. times(1) makes any second read a failure.
        ble.expect_read_status().times(1).returning(|| {
            Box::pin(async {
                let mut data = vec![0; 20];
                data[10] = 1; // zone1 active (byte 10)
                data[1] = 1; // suspend_watering (byte 1)
                Ok(data)
            })
        });

        mqtt.expect_publish()
            .with(
                eq("pub"),
                predicate::str::contains(r#""status":1"#)
                    .and(predicate::str::contains(r#""suspend_watering":true"#)),
            )
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));

        let payload = r#"{"cmd":"status","zone":"flower"}"#; // flower is zone 1
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_status_response_includes_suspend_watering() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        // byte 1 = 0 in the seed read -> suspend_watering must be false.
        ble.expect_read_status()
            .times(1)
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));

        mqtt.expect_publish()
            .with(
                eq("pub"),
                predicate::str::contains(r#""suspend_watering":false"#),
            )
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));

        let payload = r#"{"cmd":"status","zone":"flower"}"#;
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_on_off_response_has_no_suspend_watering() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_read_protocol_83()
            .returning(|| Box::pin(async { Ok(Second83Protocol::default()) }));
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));
        ble.expect_write_protocol_83()
            .withf(|p| p.zone1_enable_manual == 0)
            .returning(|_| Box::pin(async { Ok(()) }));

        mqtt.expect_publish()
            .with(
                eq("pub"),
                predicate::str::contains(r#""success":true"#)
                    .and(predicate::str::contains("suspend_watering").not()),
            )
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));

        let payload = r#"{"cmd":"on_off","zone":"flower","on_off":false}"#;
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_on_off_true_with_duration_seconds_runs_30_minutes() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_protocol_83()
            .returning(|| Box::pin(async { Ok(Second83Protocol::default()) }));
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));

        // 1800s = 0h 30m 0s
        ble.expect_write_protocol_86()
            .withf(|p| p.zm_hour == 0 && p.zm_minute == 30 && p.zm_second == 0)
            .returning(|_| Box::pin(async { Ok(()) }));

        ble.expect_write_protocol_83()
            .withf(|p| p.zone1_enable_manual == 1)
            .returning(|_| Box::pin(async { Ok(()) }));

        mqtt.expect_publish()
            .with(
                eq("pub"),
                predicate::str::contains(r#""success":true"#)
                    .and(predicate::str::contains("duration_seconds").not()),
            )
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));

        let payload = r#"{"cmd":"on_off","zone":"flower","on_off":true,"duration_seconds":1800}"#;
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_on_off_true_without_duration_uses_default_run_seconds() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_protocol_83()
            .returning(|| Box::pin(async { Ok(Second83Protocol::default()) }));
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));

        // default_run_seconds = 7200 -> 2h 0m 0s
        ble.expect_write_protocol_86()
            .withf(|p| p.zm_hour == 2 && p.zm_minute == 0 && p.zm_second == 0)
            .returning(|_| Box::pin(async { Ok(()) }));

        ble.expect_write_protocol_83()
            .withf(|p| p.zone1_enable_manual == 1)
            .returning(|_| Box::pin(async { Ok(()) }));

        mqtt.expect_publish()
            .with(eq("pub"), predicate::str::contains(r#""success":true"#))
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));

        let payload = r#"{"cmd":"on_off","zone":"flower","on_off":true}"#;
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_on_off_true_with_out_of_range_duration_yields_success_false() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_protocol_83()
            .returning(|| Box::pin(async { Ok(Second83Protocol::default()) }));
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));

        // No write_protocol_86 expected: overflow returns Ok(false) before any write.
        mqtt.expect_publish()
            .with(eq("pub"), predicate::str::contains(r#""success":false"#))
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));

        let payload =
            r#"{"cmd":"on_off","zone":"flower","on_off":true,"duration_seconds":999999999}"#;
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn test_on_off_false_with_duration_stops_immediately() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_protocol_83()
            .returning(|| Box::pin(async { Ok(Second83Protocol::default()) }));
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));

        ble.expect_write_protocol_83()
            .withf(|p| p.zone1_enable_manual == 0)
            .returning(|_| Box::pin(async { Ok(()) }));

        mqtt.expect_publish()
            .with(eq("pub"), predicate::str::contains(r#""success":true"#))
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let app = App::new(mock_config(), Arc::new(ble), Arc::new(mqtt), Arc::new(db));

        let payload = r#"{"cmd":"on_off","zone":"flower","on_off":false,"duration_seconds":3600}"#;
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
        let read_zone1_count = Arc::new(AtomicUsize::new(0));
        let read_zone2_count = Arc::new(AtomicUsize::new(0));
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
        let z1c = read_zone1_count.clone();
        ble.expect_read_zone1_name().returning(move || {
            z1c.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok("Zone 1".to_string()) })
        });
        let z2c = read_zone2_count.clone();
        ble.expect_read_zone2_name().returning(move || {
            z2c.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok("Zone 2".to_string()) })
        });

        let ble = Arc::new(ble);
        let config = mock_config();

        let (stop_tx, stop_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = watch::channel(false);
        let (status_tx, _status_rx) = watch::channel(None);
        let (zone_names_tx, _zone_names_rx) = watch::channel(None);
        let ready_check = ready_rx.clone();

        let app = Arc::new(
            App::new(config.clone(), ble.clone(), Arc::new(mqtt), Arc::new(db))
                .with_connection_ready(ready_rx),
        );

        let supervisor_ble = ble.clone();
        let supervisor_config = config;
        let controller = Arc::new(MockResilienceController::new());
        let store = ResilienceStateStore::with_path(std::env::temp_dir().join(format!(
            "rshunterbtt-sup-{}-{}.json",
            std::process::id(),
            connect_count.load(Ordering::SeqCst)
        )));
        tokio::spawn(run_connection_supervisor(
            supervisor_ble,
            supervisor_config,
            stop_rx,
            ready_tx,
            status_tx,
            zone_names_tx,
            controller,
            store,
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
                && read_zone1_count.load(Ordering::SeqCst) >= 2
                && read_zone2_count.load(Ordering::SeqCst) >= 2
        })
        .await;

        let _ = stop_tx.send(true);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    fn temp_state_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("rshunterbtt-{}-{}.json", name, std::process::id()))
    }

    #[tokio::test]
    async fn test_ladder_power_cycles_once_then_rate_limited() {
        let mut controller = MockResilienceController::new();
        let power_cycle_count = Arc::new(AtomicUsize::new(0));
        let pc = power_cycle_count.clone();
        controller.expect_power_cycle_adapter().returning(move || {
            pc.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        });
        controller
            .expect_reboot_host()
            .returning(|| Box::pin(async { Ok(()) }));

        let path = temp_state_path("powercycle");
        let store = ResilienceStateStore::with_path(&path);
        let mut ladder = ResilienceLadder::new();
        let err = anyhow!("connect failed");

        for _ in 0..5 {
            handle_connection_failure(
                &mut ladder,
                &controller,
                &store,
                &err,
                Duration::from_secs(1),
            )
            .await;
        }
        assert_eq!(
            power_cycle_count.load(Ordering::SeqCst),
            1,
            "exactly one power cycle after 5 consecutive failures"
        );

        // 6th failure occurs within the power-cycle rate-limit window -> no 2nd call.
        handle_connection_failure(
            &mut ladder,
            &controller,
            &store,
            &err,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            power_cycle_count.load(Ordering::SeqCst),
            1,
            "6th failure within rate-limit window must not power cycle again"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_ladder_reboots_after_15_failures_and_persists() {
        let mut controller = MockResilienceController::new();
        let reboot_count = Arc::new(AtomicUsize::new(0));
        let rc = reboot_count.clone();
        controller.expect_reboot_host().returning(move || {
            rc.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        });
        controller
            .expect_power_cycle_adapter()
            .returning(|| Box::pin(async { Ok(()) }));

        let path = temp_state_path("reboot");
        let store = ResilienceStateStore::with_path(&path);
        let mut ladder = ResilienceLadder::new();
        let err = anyhow!("connect failed");

        for _ in 0..15 {
            handle_connection_failure(
                &mut ladder,
                &controller,
                &store,
                &err,
                Duration::from_secs(1),
            )
            .await;
        }
        assert_eq!(
            reboot_count.load(Ordering::SeqCst),
            1,
            "reboot after 15 failures"
        );

        // State must be persisted (write+flush+fsync) before the reboot.
        let loaded = store.load().unwrap();
        assert_eq!(
            loaded.reboot_timestamps.len(),
            1,
            "reboot timestamp persisted"
        );

        let _ = std::fs::remove_file(&path);
    }

    fn temp_zone_state_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rshunterbtt-zone-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn spawn_supervisor_with_zone_names(
        ble: Arc<MockBleClient>,
        config: Config,
        ready_tx: watch::Sender<bool>,
        status_tx: StatusCacheSender,
        zone_names_tx: ZoneNamesCacheSender,
    ) -> watch::Sender<bool> {
        let (stop_tx, stop_rx) = watch::channel(false);
        let controller = Arc::new(MockResilienceController::new());
        let store = ResilienceStateStore::with_path(temp_zone_state_path());
        tokio::spawn(run_connection_supervisor(
            ble,
            config,
            stop_rx,
            ready_tx,
            status_tx,
            zone_names_tx,
            controller,
            store,
        ));
        stop_tx
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_zone_names_appear_in_status_response() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        ble.expect_connect()
            .returning(|_| Box::pin(async { Ok(()) }));
        ble.expect_write_password()
            .returning(|_| Box::pin(async { Ok(()) }));
        ble.expect_subscribe_notifications().returning(|_| {
            let (_, rx) = tokio::sync::mpsc::channel(1);
            Box::pin(async move { Ok(rx) })
        });
        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_zone1_name()
            .returning(|| Box::pin(async { Ok("Front Lawn".to_string()) }));
        ble.expect_read_zone2_name()
            .returning(|| Box::pin(async { Ok("Back Yard".to_string()) }));
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));

        mqtt.expect_publish()
            .with(
                eq("pub"),
                predicate::str::contains(r#""zone1_name":"Front Lawn""#)
                    .and(predicate::str::contains(r#""zone2_name":"Back Yard""#)),
            )
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let ble = Arc::new(ble);
        let config = mock_config();
        let (ready_tx, ready_rx) = watch::channel(false);
        let (status_tx, status_rx) = watch::channel(None);
        let (zone_names_tx, zone_names_rx) = watch::channel(None);

        let app = Arc::new(
            App::new(config.clone(), ble.clone(), Arc::new(mqtt), Arc::new(db))
                .with_connection_ready(ready_rx)
                .with_status_cache(status_rx, status_tx.clone())
                .with_zone_names(zone_names_rx),
        );

        let stop_tx =
            spawn_supervisor_with_zone_names(ble, config, ready_tx, status_tx, zone_names_tx);

        wait_until(|| {
            app.zone_names
                .borrow()
                .as_ref()
                .map(|z| z.zone1.is_some())
                .unwrap_or(false)
        })
        .await;

        let payload = r#"{"cmd":"status","zone":"flower"}"#;
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();

        let _ = stop_tx.send(true);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_zone_name_read_failure_omits_field_and_keeps_connection() {
        let mut ble = MockBleClient::new();
        let mut mqtt = MockMqttClient::new();
        let db = MockDatabaseWriter::new();

        let connect_count = Arc::new(AtomicUsize::new(0));
        let cc = connect_count.clone();
        ble.expect_connect().returning(move |_| {
            cc.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        });
        ble.expect_write_password()
            .returning(|_| Box::pin(async { Ok(()) }));
        ble.expect_subscribe_notifications().returning(|_| {
            let (_, rx) = tokio::sync::mpsc::channel(1);
            Box::pin(async move { Ok(rx) })
        });
        ble.expect_is_connected()
            .returning(|| Box::pin(async { true }));
        ble.expect_read_zone1_name()
            .returning(|| Box::pin(async { Err(anyhow!("read failed")) }));
        ble.expect_read_zone2_name()
            .returning(|| Box::pin(async { Err(anyhow!("read failed")) }));
        ble.expect_read_status()
            .returning(|| Box::pin(async { Ok(vec![0; 20]) }));

        mqtt.expect_publish()
            .with(
                eq("pub"),
                predicate::str::contains("zone1_name")
                    .not()
                    .and(predicate::str::contains("zone2_name").not()),
            )
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let ble = Arc::new(ble);
        let config = mock_config();
        let (ready_tx, ready_rx) = watch::channel(false);
        let (status_tx, status_rx) = watch::channel(None);
        let (zone_names_tx, zone_names_rx) = watch::channel(None);

        let app = Arc::new(
            App::new(config.clone(), ble.clone(), Arc::new(mqtt), Arc::new(db))
                .with_connection_ready(ready_rx)
                .with_status_cache(status_rx, status_tx.clone())
                .with_zone_names(zone_names_rx),
        );

        let stop_tx =
            spawn_supervisor_with_zone_names(ble, config, ready_tx, status_tx, zone_names_tx);

        // Wait until the supervisor has finished its best-effort zone name reads
        // (channel populated with None values) without tearing down the connection.
        wait_until(|| app.zone_names.borrow().is_some()).await;

        let payload = r#"{"cmd":"status","zone":"flower"}"#;
        app.handle_mqtt_message(payload.as_bytes()).await.unwrap();

        assert_eq!(
            connect_count.load(Ordering::SeqCst),
            1,
            "zone name read failure must not tear down the connection or reconnect"
        );

        let _ = stop_tx.send(true);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

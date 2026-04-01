use super::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Error, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use cypher_core::changelog::{ChangeEvent, CHANGELOG_STREAM};

fn test_redis_timeouts() -> crate::RedisTimeouts {
    crate::RedisTimeouts {
        connect_timeout: std::time::Duration::from_secs(1),
        read_timeout: std::time::Duration::from_secs(1),
        write_timeout: std::time::Duration::from_secs(1),
    }
}

fn read_resp_array(reader: &mut BufReader<TcpStream>) -> std::io::Result<Vec<String>> {
    let mut header = String::new();
    if reader.read_line(&mut header)? == 0 {
        return Err(Error::new(ErrorKind::UnexpectedEof, "connection closed"));
    }
    if !header.starts_with('*') {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("expected RESP array header, got {header:?}"),
        ));
    }

    let arg_count = header[1..]
        .trim()
        .parse::<usize>()
        .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
    let mut args = Vec::with_capacity(arg_count);

    for _ in 0..arg_count {
        let mut bulk_header = String::new();
        reader.read_line(&mut bulk_header)?;
        if !bulk_header.starts_with('$') {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("expected bulk string header, got {bulk_header:?}"),
            ));
        }
        let len = bulk_header[1..]
            .trim()
            .parse::<usize>()
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
        let mut buf = vec![0; len];
        reader.read_exact(&mut buf)?;
        let mut crlf = [0; 2];
        reader.read_exact(&mut crlf)?;
        args.push(String::from_utf8(buf).expect("redis command should be utf-8"));
    }

    Ok(args)
}

fn spawn_mock_redis() -> (
    Connection,
    Arc<Mutex<Vec<String>>>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock redis listener");
    let addr = listener.local_addr().expect("mock redis addr");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let commands_for_thread = Arc::clone(&commands);

    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept mock redis connection");
        let mut reader = BufReader::new(stream.try_clone().expect("clone mock redis stream"));
        let mut writer = stream;

        loop {
            let args = match read_resp_array(&mut reader) {
                Ok(args) => args,
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                Err(e) if e.kind() == ErrorKind::ConnectionReset => break,
                Err(e) => panic!("failed to parse mock redis command: {e}"),
            };

            let command = args
                .first()
                .map(|value| value.to_ascii_uppercase())
                .expect("redis command");
            commands_for_thread
                .lock()
                .unwrap()
                .push(logged_command(&args));

            match command.as_str() {
                "CLIENT" => {
                    writer.write_all(b"+OK\r\n").expect("write CLIENT response");
                    writer.flush().expect("flush CLIENT response");
                }
                "XADD" => {
                    writer
                        .write_all(b"-ERR simulated DLQ persistence failure\r\n")
                        .expect("write XADD error");
                    writer.flush().expect("flush XADD error");
                }
                "XACK" => break,
                other => panic!("unexpected redis command in test: {other}"),
            }
        }
    });

    let client = Client::open(format!("redis://{addr}/")).expect("build redis client");
    let conn = client.get_connection().expect("connect to mock redis");
    (conn, commands, handle)
}

fn logged_command(args: &[String]) -> String {
    let command = args
        .first()
        .map(|value| value.to_ascii_uppercase())
        .expect("redis command");
    if command == "XREADGROUP" {
        format!(
            "{command} {}",
            args.last().expect("XREADGROUP stream cursor")
        )
    } else {
        command
    }
}

fn mock_fabric() -> FabricGraphStore {
    FabricGraphStore::new(
        "http://localhost".to_string(),
        "tenant".to_string(),
        "client".to_string(),
        "secret".to_string(),
    )
}

fn shutdown_test_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct ShutdownStateGuard {
    _lock: MutexGuard<'static, ()>,
    original: bool,
}

impl ShutdownStateGuard {
    fn acquire() -> Self {
        let lock = shutdown_test_mutex()
            .lock()
            .expect("SHUTDOWN test mutex should not be poisoned");
        let original = SHUTDOWN.swap(false, Ordering::SeqCst);
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for ShutdownStateGuard {
    fn drop(&mut self) {
        SHUTDOWN.store(self.original, Ordering::SeqCst);
    }
}

fn permanently_failing_entry() -> StreamEntry {
    let event = ChangeEvent::CreateNode {
        graph: "g".to_string(),
        id: i64::MAX as u64 + 1,
        labels: Vec::new(),
        properties: serde_json::Map::new(),
    };
    let mut fields = HashMap::new();
    fields.insert(
        "e".to_string(),
        serde_json::to_string(&event).expect("serialize change event"),
    );
    (CHANGELOG_STREAM.to_string(), "1-0".to_string(), fields)
}

fn resp_bulk_string(value: &str) -> String {
    format!("${}\r\n{}\r\n", value.len(), value)
}

fn resp_array(items: Vec<String>) -> String {
    format!("*{}\r\n{}", items.len(), items.concat())
}

fn xreadgroup_single_entry_response(entry_id: &str, event_json: &str) -> String {
    resp_array(vec![resp_array(vec![
        resp_bulk_string(CHANGELOG_STREAM),
        resp_array(vec![resp_array(vec![
            resp_bulk_string(entry_id),
            resp_array(vec![resp_bulk_string("e"), resp_bulk_string(event_json)]),
        ])]),
    ])])
}

fn spawn_poisoned_pending_mock_redis(
    event_json: String,
) -> (
    Connection,
    Arc<Mutex<Vec<String>>>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock redis listener");
    let addr = listener.local_addr().expect("mock redis addr");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let commands_for_thread = Arc::clone(&commands);

    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept mock redis connection");
        let mut reader = BufReader::new(stream.try_clone().expect("clone mock redis stream"));
        let mut writer = stream;
        let mut pending_reads = 0;

        loop {
            let args = match read_resp_array(&mut reader) {
                Ok(args) => args,
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                Err(e) if e.kind() == ErrorKind::ConnectionReset => break,
                Err(e) => panic!("failed to parse mock redis command: {e}"),
            };

            let command = args
                .first()
                .map(|value| value.to_ascii_uppercase())
                .expect("redis command");
            commands_for_thread
                .lock()
                .unwrap()
                .push(logged_command(&args));

            match command.as_str() {
                "CLIENT" => {
                    writer.write_all(b"+OK\r\n").expect("write CLIENT response");
                    writer.flush().expect("flush CLIENT response");
                }
                "XREADGROUP" => {
                    pending_reads += 1;
                    assert_eq!(
                        args.last().map(String::as_str),
                        Some("0"),
                        "test must exercise only pending reads"
                    );

                    if pending_reads == 1 {
                        let response = xreadgroup_single_entry_response("1-0", &event_json);
                        writer
                            .write_all(response.as_bytes())
                            .expect("write XREADGROUP response");
                    } else {
                        writer
                            .write_all(b"-ERR unexpected second pending XREADGROUP\r\n")
                            .expect("write XREADGROUP error");
                    }
                    writer.flush().expect("flush XREADGROUP response");
                }
                "XADD" => {
                    writer
                        .write_all(b"-ERR simulated DLQ persistence failure\r\n")
                        .expect("write XADD error");
                    writer.flush().expect("flush XADD error");
                }
                "XACK" => {
                    writer.write_all(b":1\r\n").expect("write XACK response");
                    writer.flush().expect("flush XACK response");
                }
                other => panic!("unexpected redis command in test: {other}"),
            }
        }
    });

    let client = Client::open(format!("redis://{addr}/")).expect("build redis client");
    let conn = client.get_connection().expect("connect to mock redis");
    (conn, commands, handle)
}

fn spawn_poisoned_new_message_mock_redis(
    event_json: String,
) -> (
    Client,
    Connection,
    Arc<Mutex<Vec<String>>>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock redis listener");
    listener
        .set_nonblocking(true)
        .expect("set mock redis listener nonblocking");
    let addr = listener.local_addr().expect("mock redis addr");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let commands_for_thread = Arc::clone(&commands);

    let handle = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut connection_count = 0;
        let mut new_message_reads = 0;
        let mut dlq_failed_once = false;
        let mut replayed_pending_entry = false;

        while started.elapsed() < Duration::from_secs(2) {
            let (stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => panic!("failed to accept mock redis connection: {e}"),
            };
            connection_count += 1;

            let mut reader = BufReader::new(stream.try_clone().expect("clone mock redis stream"));
            let mut writer = stream;

            loop {
                let args = match read_resp_array(&mut reader) {
                    Ok(args) => args,
                    Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                    Err(e) if e.kind() == ErrorKind::ConnectionReset => break,
                    Err(e) => panic!("failed to parse mock redis command: {e}"),
                };

                let command = args
                    .first()
                    .map(|value| value.to_ascii_uppercase())
                    .expect("redis command");
                commands_for_thread
                    .lock()
                    .unwrap()
                    .push(logged_command(&args));

                match command.as_str() {
                    "CLIENT" => {
                        writer.write_all(b"+OK\r\n").expect("write CLIENT response");
                        writer.flush().expect("flush CLIENT response");
                    }
                    "XREADGROUP" if args.last().map(String::as_str) == Some("0") => {
                        if connection_count > 1 && !replayed_pending_entry {
                            replayed_pending_entry = true;
                            let response = xreadgroup_single_entry_response("1-0", &event_json);
                            writer
                                .write_all(response.as_bytes())
                                .expect("write pending XREADGROUP response");
                        } else {
                            writer.write_all(b"*-1\r\n").expect("write pending nil");
                        }
                        writer.flush().expect("flush pending response");
                    }
                    "XREADGROUP" if args.last().map(String::as_str) == Some(">") => {
                        new_message_reads += 1;
                        if new_message_reads == 1 {
                            let response = xreadgroup_single_entry_response("1-0", &event_json);
                            writer
                                .write_all(response.as_bytes())
                                .expect("write XREADGROUP response");
                        } else {
                            SHUTDOWN.store(true, Ordering::Relaxed);
                            writer
                                .write_all(b"*-1\r\n")
                                .expect("write recovered new-message nil");
                        }
                        writer.flush().expect("flush XREADGROUP response");
                    }
                    "XADD" => {
                        if dlq_failed_once {
                            writer
                                .write_all(resp_bulk_string("2-0").as_bytes())
                                .expect("write XADD success");
                            writer.flush().expect("flush XADD success");
                        } else {
                            dlq_failed_once = true;
                            writer
                                .write_all(b"-ERR simulated DLQ persistence failure\r\n")
                                .expect("write XADD error");
                            writer.flush().expect("flush XADD error");
                            break;
                        }
                    }
                    "XACK" => {
                        writer.write_all(b":1\r\n").expect("write XACK response");
                        writer.flush().expect("flush XACK response");
                    }
                    other => panic!("unexpected redis command in test: {other}"),
                }

                if SHUTDOWN.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
    });

    let client = Client::open(format!("redis://{addr}/")).expect("build redis client");
    let conn = client.get_connection().expect("connect to mock redis");
    (client, conn, commands, handle)
}

fn spawn_persistently_poisoned_new_message_mock_redis(
    event_json: String,
    shutdown_after_dlq_failures: usize,
) -> (
    Client,
    Connection,
    Arc<Mutex<Vec<String>>>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock redis listener");
    listener
        .set_nonblocking(true)
        .expect("set mock redis listener nonblocking");
    let addr = listener.local_addr().expect("mock redis addr");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let commands_for_thread = Arc::clone(&commands);

    let handle = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut delivered_new_message = false;
        let mut dlq_failures = 0;

        while started.elapsed() < Duration::from_secs(2) {
            if SHUTDOWN.load(Ordering::Relaxed) {
                break;
            }

            let (stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => panic!("failed to accept mock redis connection: {e}"),
            };

            let mut reader = BufReader::new(stream.try_clone().expect("clone mock redis stream"));
            let mut writer = stream;

            loop {
                let args = match read_resp_array(&mut reader) {
                    Ok(args) => args,
                    Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                    Err(e) if e.kind() == ErrorKind::ConnectionReset => break,
                    Err(e) => panic!("failed to parse mock redis command: {e}"),
                };

                let command = args
                    .first()
                    .map(|value| value.to_ascii_uppercase())
                    .expect("redis command");
                commands_for_thread
                    .lock()
                    .unwrap()
                    .push(logged_command(&args));

                match command.as_str() {
                    "CLIENT" => {
                        writer.write_all(b"+OK\r\n").expect("write CLIENT response");
                        writer.flush().expect("flush CLIENT response");
                    }
                    "XREADGROUP" if args.last().map(String::as_str) == Some("0") => {
                        if delivered_new_message {
                            let response = xreadgroup_single_entry_response("1-0", &event_json);
                            writer
                                .write_all(response.as_bytes())
                                .expect("write pending XREADGROUP response");
                        } else {
                            writer.write_all(b"*-1\r\n").expect("write pending nil");
                        }
                        writer.flush().expect("flush pending response");
                    }
                    "XREADGROUP" if args.last().map(String::as_str) == Some(">") => {
                        if delivered_new_message {
                            writer
                                .write_all(b"*-1\r\n")
                                .expect("write unexpected new-message nil");
                        } else {
                            delivered_new_message = true;
                            let response = xreadgroup_single_entry_response("1-0", &event_json);
                            writer
                                .write_all(response.as_bytes())
                                .expect("write XREADGROUP response");
                        }
                        writer.flush().expect("flush XREADGROUP response");
                    }
                    "XADD" => {
                        dlq_failures += 1;
                        if dlq_failures > shutdown_after_dlq_failures {
                            SHUTDOWN.store(true, Ordering::Relaxed);
                        }
                        writer
                            .write_all(b"-ERR simulated DLQ persistence failure\r\n")
                            .expect("write XADD error");
                        writer.flush().expect("flush XADD error");
                        break;
                    }
                    "XACK" => {
                        writer.write_all(b":1\r\n").expect("write XACK response");
                        writer.flush().expect("flush XACK response");
                    }
                    other => panic!("unexpected redis command in test: {other}"),
                }

                if SHUTDOWN.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
    });

    let client = Client::open(format!("redis://{addr}/")).expect("build redis client");
    let conn = client.get_connection().expect("connect to mock redis");
    (client, conn, commands, handle)
}

#[test]
fn does_not_ack_when_dlq_persistence_fails_after_retries_are_exhausted() {
    let (mut conn, commands, handle) = spawn_mock_redis();
    let fabric = mock_fabric();

    process_entries(&mut conn, &fabric, vec![permanently_failing_entry()], 1);

    drop(conn);
    handle
        .join()
        .expect("mock redis thread should exit cleanly");

    let commands: Vec<_> = commands
        .lock()
        .unwrap()
        .iter()
        .filter(|command| matches!(command.as_str(), "XADD" | "XACK"))
        .cloned()
        .collect();
    assert_eq!(
        commands,
        vec!["XADD"],
        "expected DLQ persistence failure to leave the stream entry unacked, got {commands:?}"
    );
}

#[test]
fn zero_retry_budget_does_not_silently_ack_failed_entries() {
    let (mut conn, commands, handle) = spawn_mock_redis();
    let fabric = mock_fabric();

    let batch_resolved = process_entries(&mut conn, &fabric, vec![permanently_failing_entry()], 0);

    drop(conn);
    handle
        .join()
        .expect("mock redis thread should exit cleanly");

    let commands: Vec<_> = commands
        .lock()
        .unwrap()
        .iter()
        .filter(|command| matches!(command.as_str(), "XADD" | "XACK"))
        .cloned()
        .collect();
    assert_eq!(
        commands,
        vec!["XADD"],
        "expected zero retry budget to attempt DLQ persistence without acking, got {commands:?}"
    );
    assert!(
        !batch_resolved,
        "expected zero retry budget with a permanent failure to leave the batch unresolved, got commands {commands:?}"
    );
}

#[test]
fn process_entries_stops_after_unresolved_dlq_failure_in_same_batch() {
    let (mut conn, commands, handle) = spawn_mock_redis();
    let fabric = mock_fabric();

    let batch_resolved = process_entries(
        &mut conn,
        &fabric,
        vec![
            permanently_failing_entry(),
            (
                CHANGELOG_STREAM.to_string(),
                "2-0".to_string(),
                HashMap::new(),
            ),
        ],
        1,
    );

    assert!(
        !batch_resolved,
        "expected unresolved DLQ failure to leave the batch unresolved"
    );

    drop(conn);
    handle
        .join()
        .expect("mock redis thread should exit cleanly");

    let commands: Vec<_> = commands
        .lock()
        .unwrap()
        .iter()
        .filter(|command| matches!(command.as_str(), "XADD" | "XACK"))
        .cloned()
        .collect();
    assert_eq!(
        commands,
        vec!["XADD"],
        "expected process_entries to stop after the unresolved DLQ failure and avoid acking later entries, got {commands:?}"
    );
}

#[test]
fn process_pending_stops_after_unresolved_pending_batch() {
    let entry = permanently_failing_entry();
    let event_json = entry
        .2
        .get("e")
        .cloned()
        .expect("permanently failing entry should include event payload");
    let (mut conn, commands, handle) = spawn_poisoned_pending_mock_redis(event_json);
    let fabric = mock_fabric();

    process_pending(&mut conn, "consumer-1", &fabric, 1, 1, CHANGELOG_STREAM);

    drop(conn);
    handle
        .join()
        .expect("mock redis thread should exit cleanly");

    let commands = commands.lock().unwrap().clone();
    let pending_reads = commands
        .iter()
        .filter(|command| command.as_str() == "XREADGROUP 0")
        .count();
    let dlq_writes = commands
        .iter()
        .filter(|command| command.as_str() == "XADD")
        .count();
    let acknowledgements = commands
        .iter()
        .filter(|command| command.as_str() == "XACK")
        .count();

    assert_eq!(
        pending_reads, 1,
        "expected process_pending to stop after the first unresolved pending batch, got commands {commands:?}"
    );
    assert_eq!(
        dlq_writes, 1,
        "expected one DLQ write attempt, got {commands:?}"
    );
    assert_eq!(
        acknowledgements, 0,
        "expected poisoned pending entry to remain unacked, got {commands:?}"
    );
}

#[test]
fn run_sync_loop_reconnects_after_unresolved_new_message_batch() {
    let _shutdown_guard = ShutdownStateGuard::acquire();

    let entry = permanently_failing_entry();
    let event_json = entry
        .2
        .get("e")
        .cloned()
        .expect("permanently failing entry should include event payload");
    let (client, mut conn, commands, handle) = spawn_poisoned_new_message_mock_redis(event_json);
    let fabric = mock_fabric();

    run_sync_loop(
        &client,
        &mut conn,
        "consumer-1",
        &fabric,
        1,
        1,
        1,
        vec![CHANGELOG_STREAM.to_string()],
        test_redis_timeouts(),
    );

    drop(conn);
    handle
        .join()
        .expect("mock redis thread should exit cleanly");

    let commands = commands.lock().unwrap().clone();
    let pending_reads = commands
        .iter()
        .filter(|command| command.as_str() == "XREADGROUP 0")
        .count();
    let new_message_reads = commands
        .iter()
        .filter(|command| command.as_str() == "XREADGROUP >")
        .count();
    let dlq_writes = commands
        .iter()
        .filter(|command| command.as_str() == "XADD")
        .count();
    let acknowledgements = commands
        .iter()
        .filter(|command| command.as_str() == "XACK")
        .count();

    assert_eq!(
        dlq_writes, 2,
        "expected the unresolved batch to be retried through the reconnect path, got {commands:?}"
    );
    assert_eq!(
        acknowledgements, 1,
        "expected the retried pending entry to be acked only after DLQ persistence succeeded, got {commands:?}"
    );
    assert_eq!(
        pending_reads, 3,
        "expected reconnect to restart with pending replay before continuing, got commands={commands:?}"
    );
    assert_eq!(
        new_message_reads, 2,
        "expected run_sync_loop to continue after reconnect and then observe no additional new work, got commands={commands:?}"
    );
}

#[test]
fn run_sync_loop_stops_after_bounded_unresolved_reconnect_attempts() {
    let _shutdown_guard = ShutdownStateGuard::acquire();

    let entry = permanently_failing_entry();
    let event_json = entry
        .2
        .get("e")
        .cloned()
        .expect("permanently failing entry should include event payload");
    let (client, mut conn, commands, handle) =
        spawn_persistently_poisoned_new_message_mock_redis(event_json, 3);
    let fabric = mock_fabric();

    run_sync_loop(
        &client,
        &mut conn,
        "consumer-1",
        &fabric,
        1,
        1,
        1,
        vec![CHANGELOG_STREAM.to_string()],
        test_redis_timeouts(),
    );

    drop(conn);
    handle
        .join()
        .expect("mock redis thread should exit cleanly");

    let commands = commands.lock().unwrap().clone();
    let dlq_writes = commands
        .iter()
        .filter(|command| command.as_str() == "XADD")
        .count();
    let pending_reads = commands
        .iter()
        .filter(|command| command.as_str() == "XREADGROUP 0")
        .count();

    assert!(
        dlq_writes <= 3,
        "expected run_sync_loop to stop after a bounded number of unresolved reconnect attempts, got dlq_writes={dlq_writes}, pending_reads={pending_reads}, commands={commands:?}"
    );
}

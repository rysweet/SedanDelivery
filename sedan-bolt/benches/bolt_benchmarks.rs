use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sedan_bolt::message::types::{encode_server_message, ServerMessage};
use sedan_bolt::packstream::decoder;
use sedan_bolt::packstream::encoder;
use sedan_bolt::packstream::value::BoltValue;

// ---------------------------------------------------------------------------
// PackStream encode benchmarks
// ---------------------------------------------------------------------------

fn bench_encode_int(c: &mut Criterion) {
    let val = BoltValue::Int(42);
    c.bench_function("encode_int", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(16);
            encoder::encode(black_box(&val), &mut buf);
            buf
        })
    });
}

fn bench_encode_string(c: &mut Criterion) {
    let short = BoltValue::String("hello world".to_string());
    c.bench_function("encode_string_short", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(32);
            encoder::encode(black_box(&short), &mut buf);
            buf
        })
    });

    let medium = BoltValue::String("x".repeat(100));
    c.bench_function("encode_string_100", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(128);
            encoder::encode(black_box(&medium), &mut buf);
            buf
        })
    });

    let long = BoltValue::String("x".repeat(1000));
    c.bench_function("encode_string_1k", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(1024);
            encoder::encode(black_box(&long), &mut buf);
            buf
        })
    });
}

fn bench_encode_map(c: &mut Criterion) {
    let entries: Vec<(String, BoltValue)> = (0..10)
        .map(|i| (format!("key_{}", i), BoltValue::Int(i)))
        .collect();
    let val = BoltValue::Map(entries);
    c.bench_function("encode_map_10", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(256);
            encoder::encode(black_box(&val), &mut buf);
            buf
        })
    });
}

fn bench_encode_list(c: &mut Criterion) {
    let items: Vec<BoltValue> = (0..100).map(BoltValue::Int).collect();
    let val = BoltValue::List(items);
    c.bench_function("encode_list_100", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(512);
            encoder::encode(black_box(&val), &mut buf);
            buf
        })
    });
}

fn bench_encode_structure(c: &mut Criterion) {
    // Simulate a Node structure: tag 0x4E, fields = [id, labels, properties]
    let node = BoltValue::Structure {
        tag: 0x4E,
        fields: vec![
            BoltValue::Int(123),
            BoltValue::List(vec![BoltValue::String("Person".into())]),
            BoltValue::Map(vec![
                ("name".into(), BoltValue::String("Alice".into())),
                ("age".into(), BoltValue::Int(30)),
                ("active".into(), BoltValue::Bool(true)),
            ]),
        ],
    };
    c.bench_function("encode_structure_node", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(128);
            encoder::encode(black_box(&node), &mut buf);
            buf
        })
    });
}

// ---------------------------------------------------------------------------
// PackStream decode benchmarks
// ---------------------------------------------------------------------------

fn bench_decode_int(c: &mut Criterion) {
    let mut buf = Vec::new();
    encoder::encode(&BoltValue::Int(42), &mut buf);
    c.bench_function("decode_int", |b| {
        b.iter(|| decoder::decode(black_box(&buf)).unwrap())
    });
}

fn bench_decode_string(c: &mut Criterion) {
    let mut buf = Vec::new();
    encoder::encode(&BoltValue::String("hello world".into()), &mut buf);
    c.bench_function("decode_string_short", |b| {
        b.iter(|| decoder::decode(black_box(&buf)).unwrap())
    });

    let mut buf_1k = Vec::new();
    encoder::encode(&BoltValue::String("x".repeat(1000)), &mut buf_1k);
    c.bench_function("decode_string_1k", |b| {
        b.iter(|| decoder::decode(black_box(&buf_1k)).unwrap())
    });
}

fn bench_decode_map(c: &mut Criterion) {
    let val = BoltValue::Map(vec![
        ("name".into(), BoltValue::String("Alice".into())),
        ("age".into(), BoltValue::Int(30)),
        ("active".into(), BoltValue::Bool(true)),
    ]);
    let mut buf = Vec::new();
    encoder::encode(&val, &mut buf);
    c.bench_function("decode_map_3", |b| {
        b.iter(|| decoder::decode(black_box(&buf)).unwrap())
    });
}

// ---------------------------------------------------------------------------
// Encode -> decode roundtrip benchmarks
// ---------------------------------------------------------------------------

fn bench_roundtrip(c: &mut Criterion) {
    let val = BoltValue::Map(vec![
        ("name".into(), BoltValue::String("Alice".into())),
        ("age".into(), BoltValue::Int(30)),
        (
            "scores".into(),
            BoltValue::List(vec![
                BoltValue::Float(95.5),
                BoltValue::Float(87.3),
                BoltValue::Float(91.0),
            ]),
        ),
        ("active".into(), BoltValue::Bool(true)),
    ]);
    c.bench_function("roundtrip_map_mixed", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(128);
            encoder::encode(black_box(&val), &mut buf);
            let (decoded, _) = decoder::decode(&buf).unwrap();
            decoded
        })
    });

    // Roundtrip a Node-like structure
    let node = BoltValue::Structure {
        tag: 0x4E,
        fields: vec![
            BoltValue::Int(42),
            BoltValue::List(vec![BoltValue::String("Person".into())]),
            BoltValue::Map(vec![
                ("name".into(), BoltValue::String("Bob".into())),
                ("age".into(), BoltValue::Int(25)),
            ]),
        ],
    };
    c.bench_function("roundtrip_structure_node", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(64);
            encoder::encode(black_box(&node), &mut buf);
            let (decoded, _) = decoder::decode(&buf).unwrap();
            decoded
        })
    });
}

// ---------------------------------------------------------------------------
// Message construction benchmarks
// ---------------------------------------------------------------------------

fn bench_message_construction(c: &mut Criterion) {
    // SUCCESS message
    let success_meta = vec![
        (
            "server".into(),
            BoltValue::String("sedan-bolt/0.1.0".into()),
        ),
        ("connection_id".into(), BoltValue::String("bolt-1".into())),
    ];
    let success_msg = ServerMessage::Success(success_meta);
    c.bench_function("msg_encode_success", |b| {
        b.iter(|| encode_server_message(black_box(&success_msg)))
    });

    // RECORD message
    let record_data = vec![
        BoltValue::Int(1),
        BoltValue::String("Alice".into()),
        BoltValue::Int(30),
        BoltValue::Bool(true),
    ];
    let record_msg = ServerMessage::Record(record_data);
    c.bench_function("msg_encode_record", |b| {
        b.iter(|| encode_server_message(black_box(&record_msg)))
    });

    // FAILURE message
    let failure_meta = vec![
        (
            "code".into(),
            BoltValue::String("Neo.ClientError.Statement.SyntaxError".into()),
        ),
        ("message".into(), BoltValue::String("Invalid input".into())),
    ];
    let failure_msg = ServerMessage::Failure(failure_meta);
    c.bench_function("msg_encode_failure", |b| {
        b.iter(|| encode_server_message(black_box(&failure_msg)))
    });
}

// ---------------------------------------------------------------------------
// Group and main
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_encode_int,
    bench_encode_string,
    bench_encode_map,
    bench_encode_list,
    bench_encode_structure,
    bench_decode_int,
    bench_decode_string,
    bench_decode_map,
    bench_roundtrip,
    bench_message_construction,
);
criterion_main!(benches);

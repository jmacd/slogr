// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0

use slogr::buffer::{BoundedBuf, ProtoBuffer};
use slogr::views::{AnyValueView, AttributeView, LogRecordView, RawLogRecord};
use slogr::{
    Attribute, EncodeResult, Level, LogRecord, LogRecordLayer, Logger, Sampler, StructBox,
    StructuredValue, ValueWriter, otel_debug, otel_error, otel_event, otel_info, otel_warn,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::{filter::LevelFilter, prelude::*};

static POINT: Attribute<dyn std::error::Error> = Attribute::new("point", "Current position");
static ANSWER: Attribute<u64> = Attribute::new("answer", "Answer");
static REASON: Attribute<str> = Attribute::new("reason", "Failure reason");
static READY: Attribute<bool> = Attribute::new("ready", "Readiness");
static VALUE: Attribute<str> = Attribute::new("value", "Payload");
static TAIL: Attribute<bool> = Attribute::new("tail", "Trailing field");

/// Scenario: a Tokio future logs mutable, thread-local structured state before and after an await.
/// Guarantees: the layer captures owned typed snapshots, independent of later mutations and lifetimes.
#[tokio::test(flavor = "current_thread")]
async fn tokio_events_capture_structured_snapshots() {
    struct Point(Rc<RefCell<i64>>);
    impl StructuredValue for Point {
        fn encode(&self, value: &mut dyn ValueWriter) -> EncodeResult {
            value.encode_map(&mut |map| {
                map.encode_entry("x", &mut |v| v.encode_i64(*self.0.borrow()))
            })
        }
    }

    // Dispatch is shareable even though this test emits exclusively on one thread.
    let records = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&records);
    let subscriber = tracing_subscriber::registry().with(LogRecordLayer::new(move |_, record| {
        captured.lock().unwrap().push(record);
    }));
    let state = Rc::new(RefCell::new(1));
    let point = StructBox::new(Point(Rc::clone(&state)));
    async {
        otel_info!("point.before", POINT = point.as_struct());
        tokio::task::yield_now().await;
        *state.borrow_mut() = 2;
        otel_info!("point.after", POINT = point.as_struct());
    }
    .with_subscriber(subscriber)
    .await;
    drop(point);
    *state.borrow_mut() = 99;

    let records = records.lock().unwrap();
    assert_eq!(records.len(), 2);
    for (record, expected) in records.iter().zip([1, 2]) {
        assert_eq!(record.callsite().target(), "slogr");
        assert_eq!(*record.callsite().level(), Level::INFO);
        assert_eq!(record.dropped_attributes_count, 0);
        let raw = RawLogRecord::new(&record.body_attrs_bytes);
        let attrs = raw.attributes().collect::<Vec<_>>();
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].key(), b"point");
        let value = attrs[0].value().unwrap();
        let entries = value.as_kvlist().unwrap().collect::<Vec<_>>();
        assert_eq!(entries[0].key(), b"x");
        assert_eq!(entries[0].value().unwrap().as_int64(), Some(expected));
        assert!(
            record
                .to_string()
                .contains(&format!("point={{x={expected}}}"))
        );
    }
    for (record, body) in records.iter().zip(["point.before", "point.after"]) {
        assert_eq!(
            RawLogRecord::new(&record.body_attrs_bytes)
                .body()
                .unwrap()
                .as_string(),
            Some(body.as_bytes())
        );
    }
}

/// Scenario: standalone macros use caller-owned sampling under a subscriber severity filter.
/// Guarantees: filtered loggers and rejected fields stay unevaluated; accepted events preserve levels and targets.
#[test]
fn filtering_and_sampling_remain_lazy() {
    struct Select {
        keep: bool,
        calls: usize,
    }
    impl Sampler for Select {
        fn should_sample(&mut self, _: &tracing::Metadata<'_>) -> bool {
            self.calls += 1;
            self.keep
        }
    }
    impl Logger for Select {}

    // The subscriber owns a Send + Sync sink; assertions inspect it after synchronous dispatch.
    let records = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&records);
    let subscriber = tracing_subscriber::registry()
        .with(LogRecordLayer::new(move |_, record| {
            captured.lock().unwrap().push(record);
        }))
        .with(LevelFilter::INFO);
    let evaluations = Cell::new(0);
    let mut sampler = Select {
        keep: false,
        calls: 0,
    };
    tracing::subscriber::with_default(subscriber, || {
        otel_debug!("filtered", logger: {
            evaluations.set(evaluations.get() + 1);
            &mut sampler
        });
        otel_info!({
            evaluations.set(evaluations.get() + 1);
            "not evaluated"
        }, logger: sampler);
        sampler.keep = true;
        otel_info!("accepted", logger: sampler, ANSWER = 42);
        otel_warn!("warn", logger: &mut sampler, ANSWER = 42);
        otel_error!("error", logger: sampler, REASON = "failure");
        otel_event!("filtered.dynamic", Level::TRACE);
        otel_event!("dynamic", Level::WARN, READY = true);
        otel_event!("empty", Level::INFO);
    });
    assert_eq!(sampler.calls, 4);
    assert_eq!(evaluations.get(), 0);
    let records = records.lock().unwrap();
    assert_eq!(records.len(), 5);
    for (record, (name, level)) in records.iter().zip([
        ("accepted", Level::INFO),
        ("warn", Level::WARN),
        ("error", Level::ERROR),
        ("dynamic", Level::WARN),
        ("empty", Level::INFO),
    ]) {
        assert_eq!(
            RawLogRecord::new(&record.body_attrs_bytes)
                .body()
                .unwrap()
                .as_string(),
            Some(name.as_bytes())
        );
        assert_eq!(*record.callsite().level(), level);
    }
    assert_eq!(records[2].callsite().target(), "slogr");
    assert!(records[1].to_string().contains("answer=42"));
}

/// Scenario: a caller-owned logger snapshots an oversized event under custom limits and exports it.
/// Guarantees: payload bytes stay bounded, loss is counted, and an undersized full-record encode rolls back.
#[test]
fn custom_budgets_and_atomic_export() {
    #[derive(Default)]
    struct Capture(Vec<LogRecord>);
    impl Sampler for Capture {}
    impl Logger for Capture {
        fn emit(&mut self, event: &tracing::Event<'_>, _: &tracing::Dispatch) {
            self.0.push(LogRecord::new_bounded::<16, 64>(event));
        }
    }

    let subscriber = tracing_subscriber::registry().with(LevelFilter::INFO);
    let mut capture = Capture::default();
    let large = "x".repeat(4096);
    tracing::subscriber::with_default(subscriber, || {
        otel_info!("bounded", logger: capture, VALUE = large.as_str(), TAIL = true);
    });
    assert_eq!(capture.0.len(), 1);
    let record = &capture.0[0];
    assert!(record.body_attrs_bytes.len() <= 64);
    assert_eq!(record.dropped_attributes_count, 1);
    assert!(record.to_string().contains("tail=true"));
    let shared = record.clone();
    assert_eq!(
        shared.body_attrs_bytes.as_ptr(),
        record.body_attrs_bytes.as_ptr()
    );

    let mut output = ProtoBuffer::with_capacity_and_limit(16, 16);
    output.try_extend_slice(b"prefix").unwrap();
    let result = output.try_atomic(|buf| {
        slogr::DirectLogRecordEncoder::new(buf).encode_log_record(std::time::UNIX_EPOCH, record)
    });
    assert_eq!(result, Err(slogr::EncodeFailure::Dropped));
    assert_eq!(output.as_ref(), b"prefix");

    let mut output = ProtoBuffer::with_capacity(256);
    slogr::DirectLogRecordEncoder::new(&mut output)
        .encode_log_record(std::time::UNIX_EPOCH, record)
        .unwrap();
    assert_eq!(
        RawLogRecord::new(output.as_ref()).dropped_attributes_count(),
        1
    );
    let allocation = output.as_ref().as_ptr();
    let bytes = output.into_bytes();
    assert_eq!(bytes.as_ptr(), allocation);
}

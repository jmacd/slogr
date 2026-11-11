// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0

use slogr::views::{AnyValueView, AttributeView, LogRecordView, RawLogRecord};
use slogr::{Attribute, Level, LogRecord};
use std::cell::Cell;
use std::sync::{Arc, Mutex};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;

static COUNT: Attribute<u64> = Attribute::new("count", "Number of items");
static READY: Attribute<bool> = Attribute::new("ready", "Readiness");

#[derive(Clone, Default)]
struct Capture {
    // A tracing Dispatch is shareable; the mutex lets tests inspect its captured events.
    events: Arc<Mutex<Vec<LogRecord>>>,
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        self.events.lock().unwrap().push(LogRecord::new(event));
    }
}

fn body(record: &LogRecord) -> String {
    let view = RawLogRecord::new(&record.body_attrs_bytes);
    String::from_utf8(view.body().unwrap().as_string().unwrap().to_vec()).unwrap()
}

/// Scenario: typed descriptors are imported, aliased, and qualified while native tracing emits equivalent values.
/// Guarantees: descriptor names become wire keys and scalar/structured native encodings remain unchanged.
#[test]
fn typed_attributes_match_native_encoding() {
    use slogr::{EncodeResult, StructBox, StructuredValue, ValueWriter};

    mod declared {
        use slogr::Attribute;
        pub static COUNT: Attribute<u64> = Attribute::new("item.count", "Number of items");
        pub static TEXT: Attribute<str> = Attribute::new("literal.key", "MUST NOT BE EMITTED");
    }
    use declared::COUNT as my_attribute;
    static SIGNED: Attribute<i64> = Attribute::new("signed", "Signed integer");
    static FLOAT: Attribute<f64> = Attribute::new("float", "Ratio");
    static POINT: Attribute<dyn std::error::Error> = Attribute::new("point", "Position");

    struct Point;
    impl StructuredValue for Point {
        fn encode(&self, value: &mut dyn ValueWriter) -> EncodeResult {
            value.encode_map(&mut |map| map.encode_entry("x", &mut |v| v.encode_i64(7)))
        }
    }
    let point = StructBox::new(Point);
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            message = "A body with spaces",
            item.count = 42u64,
            signed = -7i64,
            float = 1.25,
            ready = true,
            "literal.key" = String::from("temporary"),
            point = point.as_struct(),
        );
        slogr::otel_info!(
            String::from("A body with spaces"),
            my_attribute = 42,
            SIGNED = -7,
            FLOAT = 1.25,
            READY = true,
            declared::TEXT = String::from("temporary"),
            POINT = point.as_struct(),
        );
    });
    let events = capture.events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].body_attrs_bytes, events[1].body_attrs_bytes);
    assert_eq!(events[1].dropped_attributes_count, 0);
    let view = RawLogRecord::new(&events[1].body_attrs_bytes);
    assert_eq!(
        view.attributes()
            .map(|attribute| attribute.key().to_vec())
            .collect::<Vec<_>>(),
        [
            "item.count",
            "signed",
            "float",
            "ready",
            "literal.key",
            "point"
        ]
        .map(|name| name.as_bytes().to_vec()),
    );
    assert!(!events[1].to_string().contains("MUST NOT BE EMITTED"));
}

/// Scenario: bodies include empty and brace-containing strings, and attributes use message and internal-marker names.
/// Guarantees: the first string alone is the body; every declared field, even message, remains an ordinary attribute.
#[test]
fn body_and_attribute_names_are_independent() {
    static MESSAGE: Attribute<u64> = Attribute::new("message", "An ordinary numeric attribute");
    static MARKER: Attribute<str> = Attribute::new("__slogr_body", "An ordinary string attribute");
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        for text in ["", "Literal {} braces are not formatting"] {
            slogr::otel_info!(text, MESSAGE = 7, MARKER = "attribute");
        }
    });
    let events = capture.events.lock().unwrap();
    assert_eq!(events.len(), 2);
    let stack =
        slogr::__log_record_impl!(Level::INFO, "", MESSAGE = 7, MARKER = "attribute").into_record();
    for (record, expected) in events.iter().chain(std::iter::once(&stack)).zip([
        "",
        "Literal {} braces are not formatting",
        "",
    ]) {
        assert_eq!(body(record), expected);
        let view = RawLogRecord::new(&record.body_attrs_bytes);
        let attrs = view.attributes().collect::<Vec<_>>();
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].key(), b"message");
        assert_eq!(attrs[0].value().unwrap().as_int64(), Some(7));
        assert_eq!(attrs[1].key(), b"__slogr_body");
        assert_eq!(
            attrs[1].value().unwrap().as_string(),
            Some(b"attribute".as_slice())
        );
    }
}

/// Scenario: fixed and runtime level macros generate events from body-first calls.
/// Guarantees: severity, required bodies, and source locations survive without explicit name/parent syntax.
#[test]
fn levels_and_locations_are_preserved() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        slogr::otel_debug!("debug");
        slogr::otel_info!("info");
        slogr::otel_warn!("warn");
        slogr::otel_error!("error");
        for level in [
            Level::TRACE,
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR,
        ] {
            slogr::otel_event!("dynamic", level, COUNT = 1);
        }
        let expected = line!() + 1;
        slogr::otel_info!("location");
        let events = capture.events.lock().unwrap();
        let callsite = events.last().unwrap().callsite();
        assert_eq!(callsite.line(), Some(expected));
        assert_eq!(callsite.file(), Some(file!()));
        assert_eq!(callsite.target(), "slogr");
        assert!(callsite.name().starts_with("event "));
    });
    let events = capture.events.lock().unwrap();
    assert_eq!(events.len(), 10);
    assert_eq!(
        events
            .iter()
            .take(9)
            .map(|event| *event.callsite().level())
            .collect::<Vec<_>>(),
        [
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR,
            Level::TRACE,
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR
        ],
    );
    assert_eq!(
        events.iter().map(body).collect::<Vec<_>>(),
        [
            "debug", "info", "warn", "error", "dynamic", "dynamic", "dynamic", "dynamic",
            "dynamic", "location"
        ],
    );
}

#[derive(Default)]
struct GateState {
    enabled: bool,
    event_enabled: bool,
}

struct Gate(Arc<Mutex<GateState>>);

impl<S: Subscriber> Layer<S> for Gate {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }
    fn enabled(&self, _: &Metadata<'_>, _: Context<'_, S>) -> bool {
        self.0.lock().unwrap().enabled
    }
    fn event_enabled(&self, _: &Event<'_>, _: Context<'_, S>) -> bool {
        self.0.lock().unwrap().event_enabled
    }
}

/// Scenario: a cached callsite encounters changing filters and attribute expressions that log recursively.
/// Guarantees: metadata rejection skips both body and attributes, event rejection follows evaluation, and nested events survive.
#[test]
fn filtering_keeps_body_and_attributes_lazy() {
    fn emit(bodies: &Cell<usize>, values: &Cell<usize>) {
        slogr::otel_info!(
            {
                bodies.set(bodies.get() + 1);
                "outer"
            },
            COUNT = {
                values.set(values.get() + 1);
                slogr::otel_info!("inner", READY = true);
                1
            }
        );
    }
    let capture = Capture::default();
    // The test controls this state while the shareable subscriber reads it.
    let gate = Arc::new(Mutex::new(GateState::default()));
    let subscriber = tracing_subscriber::registry()
        .with(capture.clone())
        .with(Gate(Arc::clone(&gate)));
    let bodies = Cell::new(0);
    let values = Cell::new(0);
    tracing::subscriber::with_default(subscriber, || {
        emit(&bodies, &values);
        assert_eq!((bodies.get(), values.get()), (0, 0));
        gate.lock().unwrap().enabled = true;
        emit(&bodies, &values);
        assert_eq!((bodies.get(), values.get()), (1, 1));
        assert!(capture.events.lock().unwrap().is_empty());
        gate.lock().unwrap().event_enabled = true;
        emit(&bodies, &values);
        gate.lock().unwrap().enabled = false;
        emit(&bodies, &values);
    });
    assert_eq!((bodies.get(), values.get()), (2, 2));
    assert_eq!(
        capture
            .events
            .lock()
            .unwrap()
            .iter()
            .map(body)
            .collect::<Vec<_>>(),
        ["inner", "outer"]
    );
}

/// Scenario: dependency features impose a compile-time maximum event level.
/// Guarantees: disabled bodies stay unevaluated while raw stack records bypass level filtering.
#[test]
fn compile_time_level_filter() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let evaluations = Cell::new(0);
    let levels = [
        Level::TRACE,
        Level::DEBUG,
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ];
    tracing::subscriber::with_default(subscriber, || {
        for level in levels {
            slogr::otel_event!(
                {
                    evaluations.set(evaluations.get() + 1);
                    "static"
                },
                level,
                COUNT = 1
            );
        }
    });
    let expected = levels
        .into_iter()
        .filter(|level| *level <= tracing::level_filters::STATIC_MAX_LEVEL)
        .collect::<Vec<_>>();
    let events = capture.events.lock().unwrap();
    assert_eq!(evaluations.get(), expected.len());
    assert_eq!(
        events
            .iter()
            .map(|event| *event.callsite().level())
            .collect::<Vec<_>>(),
        expected
    );
    let raw = slogr::__log_record_impl!(Level::TRACE, "unfiltered", COUNT = 42);
    assert_eq!(body(&raw.into_record()), "unfiltered");
}

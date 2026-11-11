// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0

use slogr::{Level, LogRecord};
use std::cell::Cell;
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;

#[derive(Debug, PartialEq)]
struct Snapshot {
    name: &'static str,
    target: &'static str,
    level: Level,
    file: Option<&'static str>,
    module: Option<&'static str>,
    fields: Vec<&'static str>,
    values: Vec<(&'static str, &'static str, String)>,
    encoded: bytes::Bytes,
    dropped: u16,
    contextual: bool,
    root: bool,
    parent: Option<tracing::span::Id>,
}

#[derive(Default)]
struct Values(Vec<(&'static str, &'static str, String)>);

impl Visit for Values {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.push((field.name(), "i64", value.to_string()));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.push((field.name(), "u64", value.to_string()));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.push((field.name(), "f64", value.to_string()));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.push((field.name(), "bool", value.to_string()));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push((field.name(), "str", value.to_owned()));
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.0.push((field.name(), "error", value.to_string()));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push((field.name(), "debug", format!("{value:?}")));
    }
}

#[derive(Clone, Default)]
struct Capture {
    // A tracing Dispatch is shareable; the mutex lets tests inspect its captured events.
    events: Arc<Mutex<Vec<(u32, Snapshot)>>>,
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        let mut values = Values::default();
        event.record(&mut values);
        let record = LogRecord::new(event);
        self.events.lock().unwrap().push((
            metadata.line().expect("callsite line"),
            Snapshot {
                name: metadata.name(),
                target: metadata.target(),
                level: *metadata.level(),
                file: metadata.file(),
                module: metadata.module_path(),
                fields: metadata.fields().iter().map(|field| field.name()).collect(),
                values: values.0,
                encoded: record.body_attrs_bytes,
                dropped: record.dropped_attributes_count,
                contextual: event.is_contextual(),
                root: event.is_root(),
                parent: event.parent().cloned(),
            },
        ));
    }
}

impl Capture {
    fn assert_pairs(&self, count: usize) {
        let events = self.events.lock().unwrap();
        assert_eq!(events.len(), count * 2);
        for pair in events.chunks_exact(2) {
            assert_ne!(pair[0].0, 0);
            assert_ne!(pair[1].0, 0);
            assert_eq!(pair[0].1, pair[1].1);
        }
    }
}

/// Scenario: upstream and local event macros receive the same scalar, formatted, and structured fields.
/// Guarantees: field names, order, visitor dispatch, metadata, and encoded payloads remain identical.
#[test]
fn field_syntax_matches_upstream() {
    use slogr::{EncodeResult, StructBox, StructuredValue, ValueWriter};

    struct Point;
    impl StructuredValue for Point {
        fn encode(&self, value: &mut dyn ValueWriter) -> EncodeResult {
            value.encode_map(&mut |map| map.encode_entry("x", &mut |v| v.encode_i64(7)))
        }
    }
    struct Detail {
        r#type: &'static str,
    }
    let detail = Detail { r#type: "kind" };
    let point = StructBox::new(Point);
    let value = 42;
    const KEY: &str = "constant.key";
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        macro_rules! pair {
            ($($fields:tt)+) => {
                tracing::info!(name: "parity", target: "test", $($fields)+);
                slogr::otel_info!(target: "test", "parity", $($fields)+);
            };
        }
        pair!(
            signed = -7i64,
            unsigned = u64::MAX,
            float = 1.25,
            flag = true,
            text = "text"
        );
        pair!(value, ?detail.r#type, %value, r#type = "raw", dotted.r#type = 3);
        pair!(first = 1, "literal.key" = ?Some(2), { KEY } = %"constant",);
        pair!({ "literal.key" = 1, { KEY } = true }, "message {}", value);
        pair!(
            first = 1,
            text = debug(&value),
            shown = display(&value),
            "answer {value}"
        );
        pair!(point = point.as_struct(), message = point.as_struct());
        pair!(message = "");
        pair!("{}", "");
        pair!(before = 1, message = "explicit", after = 2);
    });
    capture.assert_pairs(9);
}

/// Scenario: named events use explicit, root, and contextual parents and every supported level.
/// Guarantees: local macros retain tracing parent semantics, empty-message behavior, and caller source locations.
#[test]
fn parents_levels_and_locations_match_upstream() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        let parent = tracing::info_span!("parent");
        let _entered = parent.enter();
        tracing::info!(name: "parented", target: "test", parent: &parent, ready = true);
        slogr::otel_info!(target: "test", "parented", parent: &parent, ready = true);
        tracing::info!(name: "root", target: "test", parent: None, "root message");
        slogr::otel_info!(target: "test", "root", parent: None, "root message");
        tracing::info!(name: "contextual", target: "test", message = "current");
        slogr::otel_info!(target: "test", "contextual", message = "current");

        tracing::debug!(name: "debug", target: "slogr", "");
        slogr::otel_debug!("debug");
        tracing::info!(name: "info", target: "slogr", "");
        slogr::otel_info!("info");
        tracing::warn!(name: "warn", target: "slogr", "");
        slogr::otel_warn!("warn");
        tracing::error!(name: "error", target: "slogr", "");
        slogr::otel_error!("error");
        for level in [
            Level::TRACE,
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR,
        ] {
            match level {
                Level::TRACE => tracing::trace!(name: "dynamic", target: "slogr", number = 1),
                Level::DEBUG => tracing::debug!(name: "dynamic", target: "slogr", number = 1),
                Level::INFO => tracing::info!(name: "dynamic", target: "slogr", number = 1),
                Level::WARN => tracing::warn!(name: "dynamic", target: "slogr", number = 1),
                Level::ERROR => tracing::error!(name: "dynamic", target: "slogr", number = 1),
            }
            slogr::otel_event!(level, "dynamic", number = 1);
        }
    });
    capture.assert_pairs(12);

    let subscriber = tracing_subscriber::registry().with(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        let expected = line!() + 1;
        slogr::otel_info!("location");
        let events = capture.events.lock().unwrap();
        let (line, event) = events.last().unwrap();
        assert_eq!(*line, expected);
        assert_eq!(event.file, Some(file!()));
        assert_eq!(event.module, Some(module_path!()));
    });
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

/// Scenario: a cached callsite encounters changing metadata/event filters and fields that log recursively.
/// Guarantees: metadata rejection skips field evaluation, event rejection follows evaluation, and nested events survive.
#[test]
fn filtering_and_reentrant_fields_match_upstream() {
    fn emit(evaluations: &Cell<usize>) {
        slogr::otel_info!(
            "outer",
            value = {
                evaluations.set(evaluations.get() + 1);
                slogr::otel_info!("inner", nested = true);
                1
            }
        );
    }

    let capture = Capture::default();
    // The same state is controlled by the test and read by the shareable subscriber.
    let gate = Arc::new(Mutex::new(GateState::default()));
    let subscriber = tracing_subscriber::registry()
        .with(capture.clone())
        .with(Gate(Arc::clone(&gate)));
    let evaluations = Cell::new(0);
    tracing::subscriber::with_default(subscriber, || {
        emit(&evaluations);
        assert_eq!(evaluations.get(), 0);
        gate.lock().unwrap().enabled = true;
        emit(&evaluations);
        assert_eq!(evaluations.get(), 1);
        assert!(capture.events.lock().unwrap().is_empty());
        gate.lock().unwrap().event_enabled = true;
        emit(&evaluations);
        gate.lock().unwrap().enabled = false;
        emit(&evaluations);
    });
    assert_eq!(evaluations.get(), 2);
    let events = capture.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|(_, event)| event.name)
            .collect::<Vec<_>>(),
        ["inner", "outer"]
    );
}

/// Scenario: dependency features impose a compile-time maximum event level.
/// Guarantees: local events honor tracing's static ceiling without evaluating disabled fields or filtering raw stack records.
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
                level,
                "static",
                value = {
                    evaluations.set(evaluations.get() + 1);
                    1
                }
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
            .map(|(_, event)| event.level)
            .collect::<Vec<_>>(),
        expected
    );
    let raw = slogr::__log_record_impl!(Level::TRACE, "unfiltered", answer = 42);
    assert!(!raw.as_view().body_attrs_bytes.is_empty());
}

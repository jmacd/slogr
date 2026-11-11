# slogr

An independent starting point for bounded, structured logging built on Tokio's
`tracing` ecosystem. Extracted from OpenTelemetry Arrow's internal logging
implementation; no Arrow, dataflow engine, OpenTelemetry SDK, submodules, build
script, or protobuf compiler is required.

## What is here

- Direct event encoding into `LogRecord`: static callsite metadata, a dropped
  attribute count, and one owned byte buffer of OTLP body/attribute fields.
- Heap-backed and stack-backed protobuf buffers with byte budgets and rollback.
- Typed nested maps/arrays through `StructuredValue` and `StructBox`, with
  truncation markers, per-member budgets, and a depth limit.
- Named `otel_info!`, `otel_warn!`, `otel_debug!`, `otel_error!`, and `otel_event!`
  macros, caller-owned `Sampler`/`Logger` hooks, and `raw_error!`.
- A callback subscriber layer, bounded console formatting, and direct encoding
  of a complete OTLP `LogRecord` message.

Engine entity context, telemetry registries, metrics, lifecycle events, resource
configuration, export batching, transport, global initialization, and output
queues are intentionally absent. Span fields and trace/span IDs are not captured.
The `otel_*` macro names are retained to keep this extraction close to its source.

## Locally owned macros

The public calling conventions live in `src/macros.rs`. The necessary event
macro machinery from `tracing` is copied into `src/macros/event.rs`: argument
normalization, field names and values, callsite metadata, level checks, and
event dispatch. Logging macros no longer expand through upstream tracing macros
or use its private macro-support module.

Tracing still supplies runtime types, callsite registration and interest caching,
static/runtime level ceilings, the dispatcher, and subscriber integration.
Existing `otel_*` syntax, lazy field evaluation, caller-owned sampling, parent
semantics, and direct encoding are preserved. Compatibility tests compare local
events with upstream events before future syntax changes.

Only the currently used event machinery is copied. Span macros, instrumentation
attributes, unnamed/untargeted macro forms, and the optional legacy `log` backend
are not vendored. Enabling `tracing/log` elsewhere does not add a `log` backend
to slogr's locally owned macros.

## Capture encoded records

```rust
use slogr::{LogRecordLayer, otel_info};
use tracing_subscriber::prelude::*;

let subscriber = tracing_subscriber::registry().with(LogRecordLayer::new(
    |_time, record| {
        assert_eq!(record.callsite().name(), "worker.ready");
        assert!(!record.body_attrs_bytes.is_empty());
        // Retain or enqueue the owned record here.
    },
));

tracing::subscriber::with_default(subscriber, || {
    otel_info!("worker.ready", workers = 4, message = "ready");
});
```

The callback runs synchronously on the emitting thread. The application owns
queueing, backpressure, delivery, and failure handling. In async code, scope a
subscriber to the future with `tracing::instrument::WithSubscriber` rather than
holding a thread-local default guard across an await.

## Console output

```rust
use slogr::{ConsoleWriter, RawLoggingLayer, otel_info};
use tracing_subscriber::prelude::*;

let subscriber = tracing_subscriber::registry()
    .with(RawLoggingLayer::new(ConsoleWriter::no_color()));
tracing::subscriber::with_default(subscriber, || {
    otel_info!("worker.ready", workers = 4);
});
```

All levels go to stderr. This layer writes synchronously and panics on an I/O
failure because `Layer::on_event` cannot return an error. For fallible direct
output, use `ConsoleWriter::write_log_record`, `print_log_record`, or `raw_error!`,
which return `std::io::Result`. No background writer or global state is installed.

## Structured values

```rust
use slogr::{EncodeResult, StructBox, StructuredValue, ValueWriter, otel_info};

struct Position { x: i64, y: i64 }

impl StructuredValue for Position {
    fn encode(&self, value: &mut dyn ValueWriter) -> EncodeResult {
        value.encode_map(&mut |map| {
            map.encode_entry("x", &mut |v| v.encode_i64(self.x))?;
            map.encode_entry("y", &mut |v| v.encode_i64(self.y))
        })
    }
}

let mut position = StructBox::new(Position { x: 1, y: 2 });
position.x = 3;
otel_info!("position.changed", position = position.as_struct());
```

Pass `as_struct()` directly, without `?` or `%`. The box is allocated once and
borrowed at logging callsites; each accepted event snapshots its current value.
Propagate callback failures with `?` to preserve container cutoff behavior.

## Memory and wire behavior

The default event buffer starts at 256 bytes and can grow to 2048 bytes.
`LogRecord::new_bounded::<INITIAL, LIMIT>` selects different bounds.
`StackLogRecord` encodes into a fixed 256-byte stack buffer without allocating.
Scalars retain the direct payload-only fast path; only structured values use
the dynamic writer interface.

The record owns one payload allocation, not an allocated object tree. This is
not a promise of exactly one allocator call: growth can reallocate, `Bytes`
ownership bookkeeping may allocate, and user formatting callbacks may allocate.
Borrowed formatting views also allocate lazy parser state. Console rendering
is separately bounded to 4096 bytes and may truncate text beyond that limit.

The stored bytes are a **partial** OTLP `LogRecord`, not an export request.
`DirectLogRecordEncoder` adds time, severity, event name, and dropped-attribute
count. It leaves scope/resource framing to the application. Its low-level writes
can leave partial output on failure; wrap encoding in `BoundedBuf::try_atomic`
when rollback is needed. Raw views and pre-encoded AnyValues are intended for
trusted internal bytes, not validation of arbitrary input.

## Development

```sh
cargo check
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo run --example console
```

Prost is test-only, used as an independent decoder for the extracted encoding
tests. Tokio is also test/example-only; the library itself requires no runtime.
Rust source is ASCII-only. Document tests with `Scenario:` and `Guarantees:`
comments and keep fixtures isolated from global subscriber state.

## Provenance and license

The logging implementation is Apache-2.0; the vendored tracing macro machinery
is MIT. Original OpenTelemetry and Tokio copyright notices are retained. See
`LICENSE`, `LICENSE-TRACING`, and `NOTICE` for terms, provenance, and changes.

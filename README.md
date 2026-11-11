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
- Body-first `otel_info!`, `otel_warn!`, `otel_debug!`, `otel_error!`, and
  `otel_event!` macros with static `Attribute<T>` declarations.
- Caller-owned `Sampler`/`Logger` hooks and `raw_error!`.
- A callback subscriber layer, bounded console formatting, and direct encoding
  of a complete OTLP `LogRecord` message.

Engine entity context, telemetry registries, metrics, lifecycle events, resource
configuration, export batching, transport, global initialization, and output
queues are intentionally absent. Span fields and trace/span IDs are not captured.
The `otel_*` macro names are retained to keep this extraction close to its source.

## Locally owned macros

The public calling conventions live in `src/macros.rs`. The event machinery in
`src/macros/event.rs` is adapted from `tracing`: callsite metadata, level checks,
borrowed field values, and event dispatch. Logging macros do not expand through
upstream tracing macros or use its private macro-support module.

Tracing still supplies runtime types, callsite registration and interest caching,
static/runtime level ceilings, the dispatcher, and subscriber integration.
Body and attribute expressions are evaluated only after filtering and sampling.
Span macros, instrumentation attributes, and the optional legacy `log` backend
are not vendored. Enabling `tracing/log` elsewhere does not add a `log` backend.

## Typed attributes and body-first syntax

```rust
use slogr::{Attribute, otel_info};

mod attributes {
    use slogr::Attribute;
    pub static COUNT: Attribute<u64> =
        Attribute::new("item.count", "Number of received items");
}
use attributes::COUNT as my_attribute;

let count: u64 = 42;
otel_info!("Items received", my_attribute = count);
```

The required first argument is one string body, not an event name or format
string. It can be a literal, `&str`, or `String` expression. Braces are literal;
there are no trailing format arguments. Tracing event names are generated from
source locations, and targets default to the emitting package name.

Every attribute key must resolve to an `Attribute<T>` declaration, using an
imported name or a qualified Rust path. Its declared name becomes the wire key;
its description is stored but not emitted. Values must borrow as `T` through
`Borrow<T>` and are encoded natively. `Attribute<str>` accepts `&str` and `String`;
numeric types are not implicitly converted. Wrong types and undeclared keys
are compile errors, even for filtered events.

There are no shorthand fields, free-form string keys, `?`/`%` overrides, or
`name:`, `parent:`, and `target:` options. `message = ...` is not a body shortcut:
it works only if `message` resolves to a declared attribute, and is then an
ordinary attribute. The type contract follows the descriptor, not a global
registry of attribute names.

For sampling, use `otel_info!(body, logger: logger, ATTRIBUTE = value)`.
For runtime severity, use `otel_event!(body, level, ATTRIBUTE = value)`.
`raw_error!(body, ATTRIBUTE = value)` uses the same typed syntax without filtering.
Ordinary third-party tracing events remain supported by the subscriber layer
with their native message convention; slogr's type checks apply to its own macros.

## Capture encoded records

```rust
use slogr::{Attribute, LogRecordLayer, otel_info};
use tracing_subscriber::prelude::*;

static WORKERS: Attribute<u64> = Attribute::new("workers", "Worker count");
let subscriber = tracing_subscriber::registry().with(LogRecordLayer::new(
    |_time, record| {
        assert!(!record.body_attrs_bytes.is_empty());
        // Retain or enqueue the owned record here.
    },
));

tracing::subscriber::with_default(subscriber, || {
    otel_info!("Workers ready", WORKERS = 4);
});
```

The callback runs synchronously on the emitting thread. The application owns
queueing, backpressure, delivery, and failure handling. In async code, scope a
subscriber to the future with `tracing::instrument::WithSubscriber` rather than
holding a thread-local default guard across an await.

## Console output

```rust
use slogr::{Attribute, ConsoleWriter, RawLoggingLayer, otel_info};
use tracing_subscriber::prelude::*;

static WORKERS: Attribute<u64> = Attribute::new("workers", "Worker count");
let subscriber = tracing_subscriber::registry()
    .with(RawLoggingLayer::new(ConsoleWriter::no_color()));
tracing::subscriber::with_default(subscriber, || {
    otel_info!("Workers ready", WORKERS = 4);
});
```

All levels go to stderr. This layer writes synchronously and panics on an I/O
failure because `Layer::on_event` cannot return an error. For fallible direct
output, use `ConsoleWriter::write_log_record`, `print_log_record`, or `raw_error!`,
which return `std::io::Result`. No background writer or global state is installed.

## Structured values

```rust
use slogr::{Attribute, EncodeResult, StructBox, StructuredValue, ValueWriter, otel_info};

static POSITION: Attribute<dyn std::error::Error> =
    Attribute::new("position", "Current position");

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
otel_info!("Position changed", POSITION = position.as_struct());
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

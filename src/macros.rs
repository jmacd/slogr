// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0
// Adapted for slogr; see NOTICE for provenance and extraction changes.

//! Named tracing events and optional caller-owned sampling.

#![allow(unused_macros)]

use tracing::{Dispatch, Event, Metadata};

#[doc(hidden)]
pub mod _private {
    pub use tracing::callsite::{Callsite, DefaultCallsite};
    pub use tracing::dispatcher::get_default;
    pub use tracing::field::ValueSet;
    pub use tracing::metadata::Kind;
    pub use tracing::{Event, Level};
    pub use tracing::{
        callsite2, debug, error, info, level_enabled, trace, valueset, valueset_all, warn,
    };

    /// Compile-time validator for OpenTelemetry event names used by the
    /// `otel_info!` / `otel_warn!` / `otel_debug!` / `otel_error!` /
    /// `otel_event!` macros.
    ///
    /// Event names must be short, stable identifiers (e.g.
    /// `receiver.start`, `channel.full`) -- not free-form sentences. This
    /// function is called from a `const` context so any violation surfaces
    /// as a compile error at the macro call site, not at runtime.
    ///
    /// Rules enforced:
    /// - non-empty
    /// - no ASCII whitespace (space, tab, newline, carriage return)
    pub const fn validate_event_name(name: &str) {
        let bytes = name.as_bytes();
        assert!(!bytes.is_empty(), "otel event name must not be empty");
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            assert!(
                b != b' ' && b != b'\t' && b != b'\n' && b != b'\r',
                "otel event name must not contain whitespace; use a short stable identifier (e.g. \"component.action\")"
            );
            i += 1;
        }
    }
}

/// Logs sampler decides when to accept a `tracing` event using only
/// the metadata, before the event is fully evaluated. May be
/// stateful.
pub trait Sampler {
    /// Observes one enabled log event and decides to keep or drop.
    /// By default, keep.
    fn should_sample(&mut self, _metadata: &Metadata<'_>) -> bool {
        true
    }
}

/// Logger controls how and where a selected event is delivered. May be
/// stateful.
pub trait Logger {
    /// Handles one selected event.
    /// By default, forward to the active subscriber.
    fn emit(&mut self, event: &Event<'_>, dispatch: &Dispatch) {
        dispatch.event(event);
    }
}

impl<S: Sampler + ?Sized> Sampler for &mut S {
    fn should_sample(&mut self, metadata: &Metadata<'_>) -> bool {
        (**self).should_sample(metadata)
    }
}

impl<L: Logger + ?Sized> Logger for &mut L {
    fn emit(&mut self, event: &Event<'_>, dispatch: &Dispatch) {
        (**self).emit(event, dispatch);
    }
}

/// Macro for logging informational messages.
///
/// # Arguments:
/// - First argument (required): The OpenTelemetry Event name identifying the log event.
///   See [OpenTelemetry Event name specification](https://github.com/open-telemetry/opentelemetry-specification/blob/v1.50.0/specification/logs/data-model.md#field-eventname).
/// - Additional optional key-value pairs can be passed as attributes.
///
/// # Example:
/// ```
/// use slogr::otel_info;
/// otel_info!("receiver.start", version = "1.0.0");
/// ```
#[macro_export]
macro_rules! otel_info {
    (logger: $logger:expr, $name:expr $(, $($fields:tt)+)?) => {{
        $crate::otel_info!(target: env!("CARGO_PKG_NAME"), logger: $logger, $name $(, $($fields)+)?)
    }};
    (target: $target:expr, logger: $logger:expr, $name:expr $(, $($fields:tt)+)?) => {{
        $crate::__otel_logger_event!(
            target: $target, logger: $logger,
            $crate::_private::Level::INFO, $name $(, $($fields)+)?
        )
    }};
    (target: $target:expr, $name:expr, $($fields:tt)+) => {{
        const _: () = $crate::_private::validate_event_name($name);
        $crate::_private::info!(name: $name, target: $target, $($fields)+);
    }};
    (target: $target:expr, $name:expr) => {{
        const _: () = $crate::_private::validate_event_name($name);
        $crate::_private::info!(name: $name, target: $target, "");
    }};
    ($name:expr, $($fields:tt)+) => {{
        $crate::otel_info!(target: env!("CARGO_PKG_NAME"), $name, $($fields)+);
    }};
    // The trailing "" is required because tracing macros need at least a format
    // string or fields. The encoder skips empty strings so no body is encoded.
    // See: `DirectFieldVisitor::record_str` in encoder.rs.
    ($name:expr) => {{
        $crate::otel_info!(target: env!("CARGO_PKG_NAME"), $name);
    }};
}

/// Macro for logging warning messages.
///
/// # Arguments:
/// - First argument (required): The OpenTelemetry Event name identifying the log event.
///   See [OpenTelemetry Event name specification](https://github.com/open-telemetry/opentelemetry-specification/blob/v1.50.0/specification/logs/data-model.md#field-eventname).
/// - Additional optional key-value pairs can be passed as attributes.
///
/// # Example:
/// ```
/// use slogr::otel_warn;
/// otel_warn!("channel.full", dropped_count = 10);
/// ```
#[macro_export]
macro_rules! otel_warn {
    (logger: $logger:expr, $name:expr $(, $($fields:tt)+)?) => {{
        $crate::otel_warn!(target: env!("CARGO_PKG_NAME"), logger: $logger, $name $(, $($fields)+)?)
    }};
    (target: $target:expr, logger: $logger:expr, $name:expr $(, $($fields:tt)+)?) => {{
        $crate::__otel_logger_event!(
            target: $target, logger: $logger,
            $crate::_private::Level::WARN, $name $(, $($fields)+)?
        )
    }};
    (target: $target:expr, $name:expr, $($fields:tt)+) => {{
        const _: () = $crate::_private::validate_event_name($name);
        $crate::_private::warn!(name: $name, target: $target, $($fields)+);
    }};
    (target: $target:expr, $name:expr) => {{
        const _: () = $crate::_private::validate_event_name($name);
        $crate::_private::warn!(name: $name, target: $target, "");
    }};
    ($name:expr, $($fields:tt)+) => {{
        $crate::otel_warn!(target: env!("CARGO_PKG_NAME"), $name, $($fields)+);
    }};
    // See otel_info! for why "" is needed here.
    ($name:expr) => {{
        $crate::otel_warn!(target: env!("CARGO_PKG_NAME"), $name);
    }};
}

/// Macro for logging debug messages.
///
/// # Arguments:
/// - First argument (required): The OpenTelemetry Event name identifying the log event.
///   See [OpenTelemetry Event name specification](https://github.com/open-telemetry/opentelemetry-specification/blob/v1.50.0/specification/logs/data-model.md#field-eventname).
/// - Additional optional key-value pairs can be passed as attributes.
///
/// # Example:
/// ```
/// use slogr::otel_debug;
/// otel_debug!("processing.batch", batch_size = 100);
/// ```
#[macro_export]
macro_rules! otel_debug {
    (logger: $logger:expr, $name:expr $(, $($fields:tt)+)?) => {{
        $crate::otel_debug!(target: env!("CARGO_PKG_NAME"), logger: $logger, $name $(, $($fields)+)?)
    }};
    (target: $target:expr, logger: $logger:expr, $name:expr $(, $($fields:tt)+)?) => {{
        $crate::__otel_logger_event!(
            target: $target, logger: $logger,
            $crate::_private::Level::DEBUG, $name $(, $($fields)+)?
        )
    }};
    (target: $target:expr, $name:expr, $($fields:tt)+) => {{
        const _: () = $crate::_private::validate_event_name($name);
        $crate::_private::debug!(name: $name, target: $target, $($fields)+);
    }};
    (target: $target:expr, $name:expr) => {{
        const _: () = $crate::_private::validate_event_name($name);
        $crate::_private::debug!(name: $name, target: $target, "");
    }};
    ($name:expr, $($fields:tt)+) => {{
        $crate::otel_debug!(target: env!("CARGO_PKG_NAME"), $name, $($fields)+);
    }};
    // See otel_info! for why "" is needed here.
    ($name:expr) => {{
        $crate::otel_debug!(target: env!("CARGO_PKG_NAME"), $name);
    }};
}

/// Macro for logging error messages.
///
/// # Arguments:
/// - First argument (required): The OpenTelemetry Event name identifying the log event.
///   See [OpenTelemetry Event name specification](https://github.com/open-telemetry/opentelemetry-specification/blob/v1.50.0/specification/logs/data-model.md#field-eventname).
/// - Additional optional key-value pairs can be passed as attributes.
///
/// # Example:
/// ```
/// use slogr::otel_error;
/// otel_error!("export.failure", error_code = 500);
/// ```
#[macro_export]
macro_rules! otel_error {
    (logger: $logger:expr, $name:expr $(, $($fields:tt)+)?) => {{
        $crate::otel_error!(target: env!("CARGO_PKG_NAME"), logger: $logger, $name $(, $($fields)+)?)
    }};
    (target: $target:expr, logger: $logger:expr, $name:expr $(, $($fields:tt)+)?) => {{
        $crate::__otel_logger_event!(
            target: $target, logger: $logger,
            $crate::_private::Level::ERROR, $name $(, $($fields)+)?
        )
    }};
    (target: $target:expr, $name:expr, $($fields:tt)+) => {{
        const _: () = $crate::_private::validate_event_name($name);
        $crate::_private::error!(name: $name, target: $target, $($fields)+);
    }};
    (target: $target:expr, $name:expr) => {{
        const _: () = $crate::_private::validate_event_name($name);
        $crate::_private::error!(name: $name, target: $target, "");
    }};
    ($name:expr, $($fields:tt)+) => {{
        $crate::otel_error!(target: env!("CARGO_PKG_NAME"), $name, $($fields)+);
    }};
    // See otel_info! for why "" is needed here.
    ($name:expr) => {{
        $crate::otel_error!(target: env!("CARGO_PKG_NAME"), $name);
    }};
}

/// Macro for logging messages at a runtime-selectable severity level.
///
/// This macro accepts a [`tracing::Level`] value as its first argument, allowing the
/// severity to be determined dynamically (e.g., from a variable or computed expression).
/// Internally it dispatches to the appropriate level-specific macro, so each invocation
/// produces one static callsite per supported level rather than requiring separate
/// `otel_debug!`/`otel_info!`/`otel_warn!`/`otel_error!` call sites.
///
/// # Arguments:
/// - First argument (required): A [`tracing::Level`] value.
/// - Second argument (required): The OpenTelemetry Event name identifying the log event.
///   See [OpenTelemetry Event name specification](https://github.com/open-telemetry/opentelemetry-specification/blob/v1.50.0/specification/logs/data-model.md#field-eventname).
/// - Additional optional key-value pairs can be passed as attributes.
///
/// # Example:
/// ```
/// use slogr::otel_event;
/// use tracing::Level;
///
/// let level = Level::DEBUG;
/// otel_event!(level, "processor.query_output", component = "my_processor");
///
/// otel_event!(Level::WARN, "channel.full", dropped_count = 10);
/// ```
#[macro_export]
macro_rules! otel_event {
    (target: $target:expr, $level:expr, $name:expr, $($fields:tt)+) => {{
        const _: () = $crate::_private::validate_event_name($name);
        match $level {
            $crate::_private::Level::TRACE => {
                $crate::_private::trace!(name: $name, target: $target, $($fields)+);
            }
            $crate::_private::Level::DEBUG => {
                $crate::_private::debug!(name: $name, target: $target, $($fields)+);
            }
            $crate::_private::Level::INFO => {
                $crate::_private::info!(name: $name, target: $target, $($fields)+);
            }
            $crate::_private::Level::WARN => {
                $crate::_private::warn!(name: $name, target: $target, $($fields)+);
            }
            $crate::_private::Level::ERROR => {
                $crate::_private::error!(name: $name, target: $target, $($fields)+);
            }
        }
    }};
    (target: $target:expr, $level:expr, $name:expr) => {{
        const _: () = $crate::_private::validate_event_name($name);
        match $level {
            $crate::_private::Level::TRACE => {
                $crate::_private::trace!(name: $name, target: $target, "");
            }
            $crate::_private::Level::DEBUG => {
                $crate::_private::debug!(name: $name, target: $target, "");
            }
            $crate::_private::Level::INFO => {
                $crate::_private::info!(name: $name, target: $target, "");
            }
            $crate::_private::Level::WARN => {
                $crate::_private::warn!(name: $name, target: $target, "");
            }
            $crate::_private::Level::ERROR => {
                $crate::_private::error!(name: $name, target: $target, "");
            }
        }
    }};
    ($level:expr, $name:expr, $($fields:tt)+) => {{
        $crate::otel_event!(target: env!("CARGO_PKG_NAME"), $level, $name, $($fields)+);
    }};
    ($level:expr, $name:expr) => {{
        $crate::otel_event!(target: env!("CARGO_PKG_NAME"), $level, $name);
    }};
}

/// Constructs an event lazily through an explicit logger after tracing accepts its metadata.
#[doc(hidden)]
#[macro_export]
macro_rules! __otel_logger_event {
    (target: $target:expr, logger: $logger:expr, $level:expr, $name:expr $(, $($fields:tt)+)?) => {{
        const _: () = $crate::_private::validate_event_name($name);
        use $crate::_private::Callsite;

        if $crate::_private::level_enabled!($level) {
            static __CALLSITE: $crate::_private::DefaultCallsite = $crate::_private::callsite2! {
                name: $name,
                kind: $crate::_private::Kind::EVENT,
                target: $target,
                level: $level,
                fields: $($($fields)+)?
            };
            let interest = __CALLSITE.interest();
            let metadata = __CALLSITE.metadata();
            if !interest.is_never()
                && (interest.is_always()
                    || $crate::_private::get_default(|dispatch| dispatch.enabled(metadata)))
            {
                // Evaluate the logger / should_sample / arguments before dispatch.
                let logger = &mut ($logger);
                if $crate::Sampler::should_sample(logger, metadata) {
                    (|values: $crate::_private::ValueSet<'_>| {
                        let event = $crate::_private::Event::new(metadata, &values);
                        $crate::_private::get_default(|dispatch| {
                            $crate::Logger::emit(logger, &event, dispatch);
                        });
                    })($crate::_private::valueset_all!(metadata.fields(), $($($fields)+)?));
                }
            }
        }
    }};
}

/// Log an error message directly to stderr, bypassing the tracing dispatcher.
///
/// Body and attributes are encoded on the stack without creating owned bytes.
/// Formatting uses borrowed views, whose lazy parser state may allocate.
/// Returns an [`std::io::Result`] so output failures are not silently discarded.
///
/// Note! the way this is written, it supports the full `tracing` syntax for
/// debug and display formatting of field values, following tracing::valueset!
/// where ? signifies debug and % signifies display.
///
/// ```no_run
/// use slogr::raw_error;
/// let err = std::io::Error::other("write failed");
/// raw_error!("logging.write.failed", error = ?err, message = %err)
///     .expect("stderr write failed");
/// ```
#[macro_export]
macro_rules! raw_error {
    ($name:expr $(, $($fields:tt)*)?) => {{
        const _: () = $crate::_private::validate_event_name($name);
        use $crate::ConsoleWriter;
        let now = std::time::SystemTime::now();
        let record = $crate::__log_record_impl!($crate::_private::Level::ERROR, $name $(, $($fields)*)?);
        ConsoleWriter::no_color().print_log_record(now, &record.as_view(), |_| {})
    }};
}

/// Internal macro that encodes a tracing event on the stack.
///
/// Returns a [`StackLogRecord`] which can be:
/// - viewed via `.as_view()` for zero-allocation formatting
/// - converted via `.into_record()` for owned `LogRecord` with `Bytes`
#[doc(hidden)]
#[macro_export]
macro_rules! __log_record_impl {
    ($level:expr, $name:expr $(, $($fields:tt)*)?) => {{
        use $crate::_private::Callsite;
        use $crate::StackLogRecord;

        static __CALLSITE: $crate::_private::DefaultCallsite = $crate::_private::callsite2! {
            name: $name,
            kind: $crate::_private::Kind::EVENT,
            target: env!("CARGO_PKG_NAME"),
            level: $level,
            fields: $($($fields)*)?
        };

        let meta = __CALLSITE.metadata();

        // Use closure to extend valueset lifetime (same pattern as tracing::event!)
        (|valueset: $crate::_private::ValueSet<'_>| {
            let event = $crate::_private::Event::new(meta, &valueset);
            StackLogRecord::new(&event)
        })($crate::_private::valueset!(meta.fields(), $($($fields)*)?))
    }};
}

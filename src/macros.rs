// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0
// Adapted for slogr; see NOTICE for provenance and extraction changes.

//! Body-first logging with declared, natively encoded attributes.

mod event;

use tracing::{Dispatch, Event, Metadata};

#[doc(hidden)]
pub mod _private {
    pub use super::event::{BODY_FIELD_NAME, body, is_enabled};
    pub use core::{concat, file, line, module_path, option::Option};
    pub use tracing::callsite::{Callsite, DefaultCallsite, Identifier};
    pub use tracing::dispatcher::get_default;
    pub use tracing::field::ValueSet;
    pub use tracing::metadata::{Kind, Metadata};
    pub use tracing::{Event, Level, field, level_filters};
}

/// Selects events using metadata before their body or attributes are evaluated.
pub trait Sampler {
    /// Observe one enabled event and decide whether to retain it.
    fn should_sample(&mut self, _metadata: &Metadata<'_>) -> bool {
        true
    }
}

/// Controls how and where a selected event is delivered.
pub trait Logger {
    /// Handle one event, forwarding to the active subscriber by default.
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

/// Log an informational event with a required string body and typed attributes.
///
/// ```
/// use slogr::{Attribute, otel_info};
/// static COUNT: Attribute<u64> = Attribute::new("item.count", "Number of items");
/// otel_info!("Items received", COUNT = 42);
/// ```
///
/// The first argument is always the body, not an event name or format string.
/// It may be a string literal, borrowed string, or `String` expression.
/// Each remaining field is `ATTRIBUTE = value`, where `ATTRIBUTE` names a
/// static [`crate::Attribute<T>`]. An optional `logger: expression` immediately
/// after the body supplies a caller-owned [`Sampler`] and [`Logger`].
///
/// There are no `message =`, `name:`, `parent:`, or `target:` shortcuts,
/// shorthand fields, free-form keys, or `?`/`%` formatting overrides.
///
/// ```compile_fail
/// slogr::otel_info!(42u64);
/// ```
///
/// ```compile_fail
/// slogr::otel_info!();
/// ```
///
/// ```compile_fail
/// slogr::otel_info!("Items received: {}", 42);
/// ```
#[macro_export]
macro_rules! otel_info {
    ($body:expr $(, $($args:tt)*)?) => {
        $crate::__slogr_log!($crate::Level::INFO, $body; $($($args)*)?)
    };
}

/// Log a warning. Uses the same body-first syntax as [`crate::otel_info!`].
#[macro_export]
macro_rules! otel_warn {
    ($body:expr $(, $($args:tt)*)?) => {
        $crate::__slogr_log!($crate::Level::WARN, $body; $($($args)*)?)
    };
}

/// Log a debug event. Uses the same body-first syntax as [`crate::otel_info!`].
#[macro_export]
macro_rules! otel_debug {
    ($body:expr $(, $($args:tt)*)?) => {
        $crate::__slogr_log!($crate::Level::DEBUG, $body; $($($args)*)?)
    };
}

/// Log an error. Uses the same body-first syntax as [`crate::otel_info!`].
#[macro_export]
macro_rules! otel_error {
    ($body:expr $(, $($args:tt)*)?) => {
        $crate::__slogr_log!($crate::Level::ERROR, $body; $($($args)*)?)
    };
}

/// Log at a runtime-selected severity, with the body first and the level second.
///
/// ```
/// use slogr::{Attribute, Level, otel_event};
/// static COUNT: Attribute<u64> = Attribute::new("item.count", "Number of items");
/// let level = Level::WARN;
/// otel_event!("Items dropped", level, COUNT = 3);
/// ```
#[macro_export]
macro_rules! otel_event {
    ($body:expr, $level:expr $(, $($args:tt)*)?) => {{
        match $level {
            $crate::Level::TRACE => $crate::__slogr_log!($crate::Level::TRACE, $body; $($($args)*)?),
            $crate::Level::DEBUG => $crate::__slogr_log!($crate::Level::DEBUG, $body; $($($args)*)?),
            $crate::Level::INFO => $crate::__slogr_log!($crate::Level::INFO, $body; $($($args)*)?),
            $crate::Level::WARN => $crate::__slogr_log!($crate::Level::WARN, $body; $($($args)*)?),
            $crate::Level::ERROR => $crate::__slogr_log!($crate::Level::ERROR, $body; $($($args)*)?),
        }
    }};
}

/// Write an error directly to stderr, bypassing filtering and sampling.
///
/// Uses the same required body and typed attributes as [`crate::otel_info!`].
/// Encoding uses a fixed stack buffer; formatting may allocate parser state.
/// Returns [`std::io::Result`] so output failures cannot be silently discarded.
///
/// ```no_run
/// use slogr::{Attribute, raw_error};
/// static CODE: Attribute<u64> = Attribute::new("error.code", "Failure code");
/// raw_error!("Write failed", CODE = 5).expect("stderr write failed");
/// ```
#[macro_export]
macro_rules! raw_error {
    ($body:expr $(, $attribute:path = $value:expr)* $(,)?) => {{
        let now = ::std::time::SystemTime::now();
        let record = $crate::__log_record_impl!($crate::Level::ERROR, $body $(, $attribute = $value)*);
        $crate::ConsoleWriter::no_color().print_log_record(now, &record.as_view(), |_| {})
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_record_impl {
    ($level:expr, $body:expr $(, $attribute:path = $value:expr)* $(,)?) => {{
        use $crate::_private::Callsite as _;
        static __CALLSITE: $crate::_private::DefaultCallsite =
            $crate::__slogr_callsite!($level; $($attribute),*);
        let metadata = __CALLSITE.metadata();
        $crate::__slogr_values!(metadata.fields(), |values: $crate::_private::ValueSet<'_>| {
            let event = $crate::_private::Event::new(metadata, &values);
            $crate::StackLogRecord::new(&event)
        }; $body; $($attribute = $value),*)
    }};
}

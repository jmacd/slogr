// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0
// Adapted for slogr; see NOTICE for provenance and extraction changes.

//! Bounded structured logging for Tokio's `tracing` ecosystem.
//!
//! [`LogRecord`] keeps static callsite metadata separate from one owned byte
//! buffer containing the OTLP-encoded body and attributes. It does not construct
//! an intermediate protobuf object tree or retain references to field values.
//! [`LogRecordLayer`] delivers these snapshots to an application-owned sink;
//! [`RawLoggingLayer`] formats them to stderr.
//!
//! Events are supported, but span fields and trace/span IDs are not captured.

#![doc = include_str!("../README.md")]

mod attribute;
pub mod buffer;
mod consts;
pub mod encoder;
pub mod formatter;
mod layer;
mod macros;
pub mod structured;
pub mod views;

#[cfg(test)]
mod test_proto;

use buffer::{ProtoBuffer, StackProtoBuffer};
use encoder::DirectFieldVisitor;
use std::fmt;
use tracing::callsite::Identifier;
use tracing::{Event, Metadata};

pub use attribute::Attribute;
pub use encoder::DirectLogRecordEncoder;
pub use formatter::{
    AnsiCode, ColorMode, ConsoleWriter, RawLoggingLayer, StyledBufWriter,
    format_log_record_to_string,
};
pub use layer::LogRecordLayer;
pub use macros::{Logger, Sampler};
pub use structured::{
    ArrayWriter, EncodeFailure, EncodeResult, MapWriter, StructBox, StructuredValue, ValueWriter,
};
pub use tracing::Level;

#[doc(hidden)]
pub use macros::_private;

/// Initial byte-buffer capacity for heap-backed event encoding.
pub const LOG_ARGUMENTS_ENCODE_INITIAL: usize = 256;
/// Maximum encoded body/attribute length for the default event constructor.
pub const LOG_ARGUMENTS_ENCODE_LIMIT: usize = 2048;
/// Fixed encoding capacity of [`StackLogRecord`].
pub const LOG_ARGUMENTS_ENCODE_STACK: usize = 256;
/// Capacity of the bounded console formatting buffer.
pub const LOG_BUFFER_SIZE: usize = LOG_ARGUMENTS_ENCODE_LIMIT * 2;

/// Structural metadata plus one owned buffer of pre-encoded body/attributes.
///
/// The timestamp belongs to the caller, not this snapshot. Cloning shares the
/// encoded bytes. Engine entity context and telemetry registries are not needed.
#[derive(Debug, Clone)]
pub struct LogRecord {
    /// Identifier of the static tracing callsite.
    pub callsite_id: Identifier,
    /// Partial OTLP LogRecord encoding: body and attributes only.
    pub body_attrs_bytes: bytes::Bytes,
    /// Number of attributes dropped or truncated to fit the encoding budget.
    pub dropped_attributes_count: u16,
}

/// Borrowed record used by both heap-backed and stack-backed formatting paths.
#[derive(Debug, Clone)]
pub struct BorrowedLogRecord<'a> {
    /// Partial OTLP LogRecord encoding.
    pub body_attrs_bytes: &'a [u8],
    /// Static callsite information.
    pub callsite: SavedCallsite,
    /// Number of dropped or truncated attributes.
    pub dropped_attributes_count: u32,
}

/// Static tracing callsite metadata, without copying strings.
#[derive(Debug, Clone)]
pub struct SavedCallsite {
    metadata: &'static Metadata<'static>,
}

impl SavedCallsite {
    /// Borrow metadata from a static tracing callsite.
    #[must_use]
    pub const fn new(metadata: &'static Metadata<'static>) -> Self {
        Self { metadata }
    }

    /// Event severity.
    #[must_use]
    pub fn level(&self) -> &Level {
        self.metadata.level()
    }

    /// Source file, if available.
    #[must_use]
    pub fn file(&self) -> Option<&'static str> {
        self.metadata.file()
    }

    /// Source line, if available.
    #[must_use]
    pub fn line(&self) -> Option<u32> {
        self.metadata.line()
    }

    /// Logical logging target.
    #[must_use]
    pub fn target(&self) -> &'static str {
        self.metadata.target()
    }

    /// Tracing event name, generated from source locations by slogr macros.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.metadata.name()
    }
}

/// A fixed-capacity, allocation-free encoding of a tracing event.
///
/// Borrow with [`Self::as_view`] or copy into owned bytes with [`Self::into_record`].
pub struct StackLogRecord {
    buf: StackProtoBuffer<LOG_ARGUMENTS_ENCODE_STACK>,
    callsite_id: Identifier,
    dropped_count: u32,
}

impl StackLogRecord {
    /// Encode body and attributes directly into stack storage.
    #[must_use]
    pub fn new(event: &Event<'_>) -> Self {
        let mut buf = StackProtoBuffer::default();
        let dropped_count = {
            let mut visitor = DirectFieldVisitor::for_event(&mut buf, event);
            event.record(&mut visitor);
            visitor.dropped_count()
        };
        Self {
            buf,
            callsite_id: event.metadata().callsite(),
            dropped_count,
        }
    }

    /// Borrow the encoded bytes without allocating.
    #[must_use]
    pub fn as_view(&self) -> BorrowedLogRecord<'_> {
        BorrowedLogRecord {
            body_attrs_bytes: self.buf.as_ref(),
            callsite: SavedCallsite::new(self.callsite_id.0.metadata()),
            dropped_attributes_count: self.dropped_count,
        }
    }

    /// Copy the stack buffer into an owned record.
    #[must_use]
    pub fn into_record(self) -> LogRecord {
        LogRecord {
            callsite_id: self.callsite_id,
            body_attrs_bytes: self.buf.to_bytes(),
            dropped_attributes_count: self.dropped_count as u16,
        }
    }
}

impl LogRecord {
    /// Encode an event using the default initial capacity and byte limit.
    #[must_use]
    pub fn new(event: &Event<'_>) -> Self {
        Self::new_bounded::<LOG_ARGUMENTS_ENCODE_INITIAL, LOG_ARGUMENTS_ENCODE_LIMIT>(event)
    }

    /// Encode into a buffer preallocated to `INITIAL`, growing up to `LIMIT`.
    ///
    /// Byte limits are clamped to [`buffer::MAX_OTLP_SIZE_LIMIT`].
    /// Oversized strings are UTF-8-safely truncated; attributes that cannot
    /// fit are dropped. Both cases increment the dropped-attribute count.
    #[must_use]
    pub fn new_bounded<const INITIAL: usize, const LIMIT: usize>(event: &Event<'_>) -> Self {
        let mut buf = ProtoBuffer::with_capacity_and_limit(INITIAL, LIMIT);
        let dropped_count = {
            let mut visitor = DirectFieldVisitor::for_event(&mut buf, event);
            event.record(&mut visitor);
            visitor.dropped_count()
        };
        Self {
            callsite_id: event.metadata().callsite(),
            body_attrs_bytes: buf.into_bytes(),
            dropped_attributes_count: dropped_count as u16,
        }
    }

    /// Access the static callsite information.
    #[must_use]
    pub fn callsite(&self) -> SavedCallsite {
        SavedCallsite::new(self.callsite_id.0.metadata())
    }

    /// Borrow this record for formatting.
    #[must_use]
    pub fn as_view(&self) -> BorrowedLogRecord<'_> {
        BorrowedLogRecord {
            body_attrs_bytes: self.body_attrs_bytes.as_ref(),
            callsite: self.callsite(),
            dropped_attributes_count: self.dropped_attributes_count as u32,
        }
    }
}

impl fmt::Display for LogRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_log_record_to_string(None, self))
    }
}

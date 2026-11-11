// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0
// Adapted for slogr; see NOTICE for provenance and extraction changes.

//! Direct OTLP bytes encoder for tokio-tracing events.

use super::structured::{
    ArrayWriter, MAX_DEPTH, MapWriter, StructCarrier, StructuredValue, ValueWriter,
};
use super::{LogRecord, SavedCallsite};
use crate::buffer::{BoundedBuf, EncodeFailure, EncodeResult};
use crate::consts::{field_num::common::*, field_num::logs::*, wire_types};
use std::time::SystemTime;
use tracing::Level;

/// Suffix appended to truncated strings.
pub const TRUNCATION_SUFFIX: &[u8] = "[\u{2020}]".as_bytes();

const DEPTH_CUTOFF: &str = "[\u{22f1}]";

// Pre-encoded ArrayValue element and KeyValueList entry with minimal framing.
const ARRAY_CUTOFF: &[u8] = "\x0a\x07\x0a\x05[\u{2020}]".as_bytes();
const MAP_CUTOFF: &[u8] = "\x0a\x10\x0a\x05[\u{2020}]\x12\x07\x0a\x05[\u{2020}]".as_bytes();

/// Direct encoder that writes a single LogRecord from a tracing Event.
pub struct DirectLogRecordEncoder<'buf, B: BoundedBuf> {
    buf: &'buf mut B,
}

impl<'buf, B: BoundedBuf> DirectLogRecordEncoder<'buf, B> {
    /// Create a new encoder that writes to the provided buffer.
    #[inline]
    pub const fn new(buf: &'buf mut B) -> Self {
        Self { buf }
    }

    /// Reset the underlying buffer.
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Encode a tracing Event as a complete LogRecord message.
    pub fn encode_log_record(&mut self, time: SystemTime, record: &LogRecord) -> EncodeResult {
        // Convert SystemTime to nanoseconds since UNIX epoch
        let timestamp_ns = time
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        // Encode time_unix_nano (fixed64)
        let _ = self
            .buf
            .encode_field_tag(LOG_RECORD_TIME_UNIX_NANO, wire_types::FIXED64);
        self.buf.try_extend_slice(&timestamp_ns.to_le_bytes())?;

        // Encode severity_number (varint)
        let severity = level_to_severity_number(record.callsite().level());
        self.buf
            .encode_field_tag(LOG_RECORD_SEVERITY_NUMBER, wire_types::VARINT)?;
        self.buf.encode_varint(severity as u64)?;

        // Note we skip encoding severity_text (string)

        bounded::record_event_name(self.buf, record.callsite())?;

        // Pre-encoded body and attributes.
        self.buf.try_extend_slice(&record.body_attrs_bytes)?;

        // Encode dropped_attributes_count (varint) if any were dropped.
        if record.dropped_attributes_count > 0 {
            self.buf
                .encode_field_tag(LOG_RECORD_DROPPED_ATTRIBUTES_COUNT, wire_types::VARINT)?;
            self.buf
                .encode_varint(record.dropped_attributes_count as u64)?;
        }

        Ok(())
    }
}

/// Truncation status.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Truncation {
    /// No truncation.
    None,
    /// Some truncation
    Some,
}

impl From<()> for Truncation {
    fn from(_: ()) -> Self {
        Truncation::None
    }
}

/// Result type for encoding methods with truncation.
pub type TruncateResult = EncodeResult<Truncation>;

// BoundedBuf support methods.
mod bounded {
    use super::{
        BoundedBufFmt, SavedCallsite, TRUNCATION_SUFFIX, TruncateResult, Truncation, ValueEncoder,
    };
    use crate::buffer::{BoundedBuf, EncodeFailure, EncodeResult};
    use crate::consts::{field_num::common::*, field_num::logs::*, wire_types};

    /// Encode the event name from callsite metadata.
    ///
    /// Emits `callsite.name()`, generated from the source location for slogr
    /// macros. This is separate from the required body string. Applications can
    /// carry the tracing target in the enclosing OTLP instrumentation scope.
    pub fn record_event_name<B: BoundedBuf>(buf: &mut B, callsite: SavedCallsite) -> EncodeResult {
        buf.encode_len_delimited(LOG_RECORD_EVENT_NAME, |buf| {
            buf.try_extend_slice(callsite.name().as_bytes())
        })
    }

    /// Encodes one top-level attribute whose AnyValue payload is written by `encode`.
    pub fn record_attribute<B: BoundedBuf>(
        buf: &mut B,
        key: &str,
        encode: impl FnOnce(&mut B) -> TruncateResult,
    ) -> TruncateResult {
        any_keyvalue(buf, LOG_RECORD_ATTRIBUTES, key, encode)
    }

    /// Encodes a body payload, maybe truncating
    pub fn record_body<B: BoundedBuf>(
        buf: &mut B,
        encode: impl FnOnce(&mut B) -> TruncateResult,
    ) -> TruncateResult {
        buf.encode_len_delimited(LOG_RECORD_BODY, encode)
    }

    /// Encodes one top-level attribute whose AnyValue payload is written by `encode`.
    pub fn any_keyvalue<B: BoundedBuf>(
        buf: &mut B,
        tag: u64,
        key: &str,
        encode: impl FnOnce(&mut B) -> TruncateResult,
    ) -> TruncateResult {
        buf.encode_len_delimited(tag, |buf| {
            buf.encode_string(KEY_VALUE_KEY, key)?;
            buf.encode_len_delimited(KEY_VALUE_VALUE, encode)
        })
    }

    /// Writes all non-length-delimited fields.
    pub fn primtiive_field<B: BoundedBuf>(
        buf: &mut B,
        tag: u64,
        wire_type: u64,
        encode: impl FnOnce(&mut B) -> EncodeResult,
    ) -> EncodeResult {
        buf.encode_field_tag(tag, wire_type)?;
        encode(buf)
    }

    /// Encodes any value as a length-delimited `tag`.
    pub fn value_encoder<B: BoundedBuf>(
        buf: &mut B,
        tag: u64,
        depth: usize,
        value: &mut impl FnMut(&mut ValueEncoder<'_, B>) -> TruncateResult,
    ) -> TruncateResult {
        buf.encode_len_delimited(tag, |buf| {
            let mut encoder = ValueEncoder::new(buf, depth);
            value(&mut encoder)
        })
    }

    /// Encodes a `KeyValue` as a length-delimited `tag`.
    pub fn key_value_encoder<B: BoundedBuf>(
        buf: &mut B,
        tag: u64,
        key: &str,
        depth: usize,
        value: &mut impl FnMut(&mut ValueEncoder<'_, B>) -> TruncateResult,
    ) -> TruncateResult {
        buf.encode_len_delimited(tag, |buf| {
            buf.encode_string(KEY_VALUE_KEY, key)?;
            value_encoder(buf, KEY_VALUE_VALUE, depth, value)
        })
    }

    #[cfg(debug_assertions)]
    pub fn is_raw_any_value(encoded: &[u8]) -> bool {
        use crate::views::decode::{read_fixed64, read_len_delim, read_varint};

        let Some((tag, pos)) = read_varint(encoded, 0) else {
            return false;
        };
        let end = match (tag >> 3, tag & 7) {
            (ANY_VALUE_BOOL_VALUE | ANY_VALUE_INT_VALUE, wire_types::VARINT) => {
                read_varint(encoded, pos).map(|(_, end)| end)
            }
            (ANY_VALUE_DOUBLE_VALUE, wire_types::FIXED64) => {
                read_fixed64(encoded, pos).map(|(_, end)| end)
            }
            (
                ANY_VALUE_STRING_VALUE
                | ANY_VALUE_ARRAY_VALUE
                | ANY_VALUE_KVLIST_VALUE
                | ANY_VALUE_BYTES_VALUE,
                wire_types::LEN,
            ) => read_len_delim(encoded, pos).map(|(_, end)| end),
            _ => None,
        };
        end == Some(encoded.len())
    }

    /// Encode a `Debug` value as a protobuf string field with truncation support.
    #[inline]
    pub fn debug_string<B: BoundedBuf>(buf: &mut B, value: &dyn std::fmt::Debug) -> TruncateResult {
        buf.encode_len_delimited(ANY_VALUE_STRING_VALUE, |buf| {
            use std::fmt::Write as _;
            let mut adapter = BoundedBufFmt::new(buf);

            // Ignore the fmt result -- truncation is handled internally by the
            // adapter; the write!() call always returns Ok because the adapter
            // silently discards overflow bytes after appending the suffix.
            let _ = write!(adapter, "{:?}", value);
            // Extract state before dropping the adapter (which borrows buf).
            let truncated = adapter.truncated;
            let content_start = adapter.content_start;
            if truncated {
                if buf.len() <= content_start {
                    // Nothing useful was written (not even the suffix fit).
                    return Err(EncodeFailure::Dropped);
                }
                return Ok(Truncation::Some);
            }
            Ok(Truncation::None)
        })
    }

    /// Encode a string field, appending the dagger suffix if it won't fit.
    pub fn string_truncating<B: BoundedBuf>(buf: &mut B, tag: u64, val: &str) -> TruncateResult {
        let full_len = val.len();
        let tag_bytes = varint_len((tag << 3) | wire_types::LEN);

        // Check if the full string fits.
        let full_field_size = tag_bytes + varint_len(full_len as u64) + full_len;
        if buf.len() + full_field_size <= buf.limit() {
            return Ok(buf.encode_string(tag, val)?.into());
        }

        let suffix_len = TRUNCATION_SUFFIX.len();
        let avail = buf.remaining();

        // Minimum encoding is: tag + 1-byte length varint + suffix bytes.
        let min_overhead = tag_bytes + 1 + suffix_len;
        if avail < min_overhead {
            return Err(EncodeFailure::Dropped);
        }

        // After the tag, we have `len_and_payload` bytes for the
        // length varint plus the payload (content + suffix). The
        // `min_overhead` check above guarantees there is room for the
        // varint and the suffix, so `content_len` is non-negative.
        let len_and_payload = avail - tag_bytes;
        let len_varint_size = varint_len(len_and_payload as u64);
        debug_assert!(
            len_and_payload >= len_varint_size + suffix_len,
            "min_overhead check guarantees varint + suffix fit",
        );
        let content_len = len_and_payload - len_varint_size - suffix_len;

        // Truncate at a UTF-8 character boundary.
        let truncated_str = truncate_utf8(val, content_len);
        let payload_len = truncated_str.len() + suffix_len;

        // Pre-validated: each write fits within the limit.
        buf.encode_field_tag(tag, wire_types::LEN)
            .expect("pre-validated fit");
        buf.encode_varint(payload_len as u64)
            .expect("pre-validated fit");
        buf.try_extend_slice(truncated_str.as_bytes())
            .expect("pre-validated fit");
        buf.try_extend_slice(TRUNCATION_SUFFIX)
            .expect("pre-validated fit");
        Ok(Truncation::Some)
    }

    /// Compute the number of bytes needed to encode a varint value.
    #[inline]
    const fn varint_len(value: u64) -> usize {
        // Each byte encodes 7 bits; we need ceil(bits_needed / 7), minimum 1.
        if value == 0 {
            return 1;
        }
        let bits = 64 - value.leading_zeros() as usize;
        bits.div_ceil(7)
    }

    /// Truncate a string to at most `max_bytes` bytes at a UTF-8 character boundary.
    pub fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
        if s.len() <= max_bytes {
            return s;
        }
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Distinguishes the body from attributes, including native tracing events.
pub enum LogFieldLocation<'a> {
    /// The body.
    Body,
    /// An attribute.
    Attribute(&'a str),
}

impl<'a> LogFieldLocation<'a> {
    fn new(field: &tracing::field::Field, body_first: bool) -> Self {
        let name = field.name();
        let is_body = if body_first {
            field.index() == 0
        } else {
            name == "message"
        };
        if is_body {
            Self::Body
        } else {
            Self::Attribute(name)
        }
    }
}

/// Visitor that directly encodes tracing fields to protobuf.
pub struct DirectFieldVisitor<'buf, B: BoundedBuf> {
    buf: &'buf mut B,
    dropped_count: u32,
    body_first: bool,
}

impl<'buf, B: BoundedBuf> DirectFieldVisitor<'buf, B> {
    /// Visit individual native tracing fields, using its `message` convention.
    ///
    /// When recording a complete event, use [`Self::for_event`] instead.
    pub const fn new(buf: &'buf mut B) -> Self {
        Self {
            buf,
            dropped_count: 0,
            body_first: false,
        }
    }

    /// Visit an event using either slogr's body-first layout or native tracing's
    /// message convention. Recognition inspects only the first metadata field.
    pub fn for_event(buf: &'buf mut B, event: &tracing::Event<'_>) -> Self {
        let mut visitor = Self::new(buf);
        visitor.body_first = event
            .metadata()
            .fields()
            .iter()
            .next()
            .is_some_and(|field| field.name() == crate::_private::BODY_FIELD_NAME);
        visitor
    }

    /// Returns the number of fields dropped due to size limits.
    #[must_use]
    pub const fn dropped_count(&self) -> u32 {
        self.dropped_count
    }

    /// Main entry point.
    fn record_toplevel_field(
        &mut self,
        field: &tracing::field::Field,
        encode: impl FnOnce(&mut B) -> TruncateResult,
    ) {
        let budget = self.buf.remaining().div_ceil(2);

        // The toplevel call ignores truncation.
        let _ = self.buf.try_atomic(|buf| {
            match LogFieldLocation::new(field, self.body_first) {
                LogFieldLocation::Body => {
                    // May be truncated, but not counted in dropped_attributes.
                    bounded::record_body(buf, encode)
                }
                LogFieldLocation::Attribute(name) => {
                    // Attributes are given only half the remaining space.
                    buf.with_max_remaining(budget, |buf| {
                        let result = bounded::record_attribute(buf, name, encode);
                        match result {
                            Ok(Truncation::None) => {}
                            _ => {
                                // Dropped or truncated
                                self.dropped_count += 1;
                            }
                        };
                        result
                    })
                }
            }
        });
    }

    fn record_structured(&mut self, field: &tracing::field::Field, value: &dyn StructuredValue) {
        self.record_toplevel_field(field, |buf| {
            ValueEncoder::new(buf, 0).encode_structured(|encoder| value.encode(encoder))
        });
    }
}

/// Adapter that lets `write!` format directly into a [`BoundedBuf`] without
/// an intermediate `String`.
///
/// When the buffer fills up, the adapter backtracks to make room for a
/// dagger truncation suffix and silently discards all subsequent writes.
/// The `truncated` flag records whether truncation occurred so callers can
/// report the partial value instead of dropping it entirely.
struct BoundedBufFmt<'a, B: BoundedBuf> {
    buf: &'a mut B,
    /// Buffer position where formatted content starts (after protobuf framing).
    content_start: usize,
    /// Set to `true` once truncation has occurred.
    truncated: bool,
}

impl<'a, B: BoundedBuf> BoundedBufFmt<'a, B> {
    /// Create a new adapter. `content_start` is recorded as the current
    /// buffer length so the backtracking logic knows which bytes are ours.
    #[inline]
    fn new(buf: &'a mut B) -> Self {
        let content_start = buf.len();
        Self {
            buf,
            content_start,
            truncated: false,
        }
    }

    /// Backtrack from the end of the buffer to make room for `TRUNCATION_SUFFIX`,
    /// append it, and mark ourselves as truncated.
    ///
    /// If there is not enough content to free space for the suffix the buffer
    /// is left at `content_start` (empty content) and the caller should treat
    /// this as a hard drop.
    fn truncate_with_suffix(&mut self) {
        let suffix_len = TRUNCATION_SUFFIX.len();
        let cur = self.buf.len();

        // Target position: back up `suffix_len` bytes from the current end,
        // then walk backwards (at most 3 extra bytes) to a UTF-8 char boundary.
        // UTF-8 continuation bytes have the bit pattern 10xxxxxx, so we skip
        // them until we land on a leading byte.
        let target = cur.saturating_sub(suffix_len).max(self.content_start);
        let mut pos = target;
        let slice = self.buf.as_slice();
        while pos > self.content_start && (slice[pos] & 0xC0) == 0x80 {
            pos -= 1;
        }
        self.buf.truncate(pos);

        // Try to append the suffix. If even this fails (degenerate budget),
        // leave the buffer at whatever position truncate() set.
        let _ = self.buf.try_extend_slice(TRUNCATION_SUFFIX);
        self.truncated = true;
    }
}

impl<B: BoundedBuf> std::fmt::Write for BoundedBufFmt<'_, B> {
    #[inline]
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        if self.truncated {
            // Already truncated -- silently discard further output so the
            // formatter finishes cleanly.
            return Ok(());
        }
        match self.buf.try_extend_slice(s.as_bytes()) {
            Ok(()) => Ok(()),
            Err(_) => {
                // The full chunk did not fit. Write as much as we can, then
                // backtrack to append the truncation suffix.
                let remaining = self.buf.remaining();
                if remaining > 0 {
                    // Find the last UTF-8 char boundary within the portion
                    // of `s` that fits.
                    let mut end = remaining;
                    while end > 0 && !s.is_char_boundary(end) {
                        end -= 1;
                    }
                    if end > 0 {
                        // This extend cannot fail -- we checked remaining.
                        let _ = self.buf.try_extend_slice(&s.as_bytes()[..end]);
                    }
                }
                self.truncate_with_suffix();
                Ok(())
            }
        }
    }
}

/// Encodes one AnyValue into a bounded buffer, used for nested encoding.
struct ValueEncoder<'a, B: BoundedBuf> {
    buf: &'a mut B,
    depth: usize,
    truncation: Truncation,

    /// ensures at most one value is written.
    #[cfg(debug_assertions)]
    single: bool,
}

impl<'a, B: BoundedBuf> ValueEncoder<'a, B> {
    fn new(buf: &'a mut B, depth: usize) -> Self {
        Self {
            buf,
            depth,
            truncation: Truncation::None,
            #[cfg(debug_assertions)]
            single: false,
        }
    }

    fn encode_structured(
        &mut self,
        f: impl FnOnce(&mut dyn ValueWriter) -> EncodeResult,
    ) -> TruncateResult {
        f(self)?;
        Ok(self.truncation)
    }

    fn once<T, E>(&mut self, f: impl FnOnce(&mut B) -> Result<T, E>) -> Result<T, E> {
        #[cfg(debug_assertions)]
        {
            debug_assert!(
                !self.single,
                "value callback must write at most one AnyValue variant"
            );
            self.single = true;
        }
        f(self.buf)
    }

    /// Encodes a length-delimited container; `body` reports lost content.
    fn encode_nested(
        &mut self,
        field: u64,
        body: impl FnOnce(&mut NestedEncoder<'_, B>) -> TruncateResult,
    ) -> TruncateResult {
        let depth = self.depth + 1;
        self.once(|buf| {
            buf.encode_len_delimited(field, |buf| {
                let mut encoder = NestedEncoder::new(buf, depth);
                body(&mut encoder)
            })
        })
    }

    /// Applies the depth and truncation policy to a structured container.
    fn encode_structured_nested(
        &mut self,
        field: u64,
        body: impl FnOnce(&mut NestedEncoder<'_, B>) -> EncodeResult,
    ) -> EncodeResult {
        self.truncation = if self.depth >= MAX_DEPTH {
            self.once(|buf| buf.encode_string(ANY_VALUE_STRING_VALUE, DEPTH_CUTOFF))?;
            Truncation::Some
        } else {
            self.encode_nested(field, |encoder| encoder.structured(field, body))?
        };
        Ok(())
    }

    fn encode_str(&mut self, value: &str) -> TruncateResult {
        self.once(|buf| bounded::string_truncating(buf, ANY_VALUE_STRING_VALUE, value))
    }

    fn encode_i64(&mut self, value: i64) -> EncodeResult {
        self.once(|buf| {
            bounded::primtiive_field(buf, ANY_VALUE_INT_VALUE, wire_types::VARINT, |buf| {
                buf.encode_varint(value as u64)
            })
        })
    }

    fn encode_f64(&mut self, value: f64) -> EncodeResult {
        self.once(|buf| {
            bounded::primtiive_field(buf, ANY_VALUE_DOUBLE_VALUE, wire_types::FIXED64, |buf| {
                buf.try_extend_slice(&value.to_le_bytes())
            })
        })
    }

    fn encode_bool(&mut self, value: bool) -> EncodeResult {
        self.once(|buf| {
            bounded::primtiive_field(buf, ANY_VALUE_BOOL_VALUE, wire_types::VARINT, |buf| {
                buf.encode_varint(u64::from(value))
            })
        })
    }
}

/// Encodes the members of a nested map or array.
struct NestedEncoder<'a, B: BoundedBuf> {
    buf: &'a mut B,
    depth: usize,
    truncation: Truncation,
    cutoff: Option<usize>,
}

impl<'a, B: BoundedBuf> NestedEncoder<'a, B> {
    fn new(buf: &'a mut B, depth: usize) -> Self {
        Self {
            buf,
            depth,
            truncation: Truncation::None,
            cutoff: None,
        }
    }

    fn remember_cutoff(&mut self, marker: &[u8]) {
        if self.buf.remaining() >= marker.len() {
            self.cutoff = Some(self.buf.len());
        }
    }

    fn structured(
        &mut self,
        field: u64,
        body: impl FnOnce(&mut NestedEncoder<'_, B>) -> EncodeResult,
    ) -> TruncateResult {
        let marker = if field == ANY_VALUE_KVLIST_VALUE {
            MAP_CUTOFF
        } else {
            ARRAY_CUTOFF
        };
        self.remember_cutoff(marker);
        match body(self) {
            Ok(()) => Ok(self.truncation),
            Err(EncodeFailure::Dropped) => {
                // Reclaim only the suffix after the last marker-safe member
                // boundary, rather than withholding space in every ancestor.
                let cutoff = self.cutoff.ok_or(EncodeFailure::Dropped)?;
                self.buf.truncate(cutoff);
                self.buf.try_extend_slice(marker)?;
                Ok(Truncation::Some)
            }
        }
    }

    fn member(&mut self, marker: &[u8], f: impl FnOnce(&mut B) -> TruncateResult) -> EncodeResult {
        let budget = self.buf.remaining().div_ceil(2);
        let truncation = self
            .buf
            .with_max_remaining(budget, |buf| buf.try_atomic(f))?;
        if truncation == Truncation::Some {
            self.truncation = Truncation::Some;
        }
        self.remember_cutoff(marker);
        Ok(())
    }
}

impl<B: BoundedBuf> ValueWriter for ValueEncoder<'_, B> {
    fn encode_str(&mut self, value: &str) -> EncodeResult {
        self.truncation = ValueEncoder::encode_str(self, value)?;
        Ok(())
    }

    fn encode_debug(&mut self, value: &dyn std::fmt::Debug) -> EncodeResult {
        self.truncation = self.once(|buf| bounded::debug_string(buf, value))?;
        Ok(())
    }

    fn encode_i64(&mut self, value: i64) -> EncodeResult {
        ValueEncoder::encode_i64(self, value)
    }

    fn encode_f64(&mut self, value: f64) -> EncodeResult {
        ValueEncoder::encode_f64(self, value)
    }

    fn encode_bool(&mut self, value: bool) -> EncodeResult {
        ValueEncoder::encode_bool(self, value)
    }

    fn encode_raw_any_value(&mut self, encoded: &[u8]) -> EncodeResult {
        #[cfg(debug_assertions)]
        debug_assert!(
            bounded::is_raw_any_value(encoded),
            "raw encoding must contain exactly one recognized AnyValue field"
        );
        self.once(|buf| buf.try_extend_slice(encoded))
    }

    fn encode_map(
        &mut self,
        f: &mut dyn FnMut(&mut dyn MapWriter) -> EncodeResult,
    ) -> EncodeResult {
        self.encode_structured_nested(ANY_VALUE_KVLIST_VALUE, |encoder| f(encoder))
    }

    fn encode_array(
        &mut self,
        f: &mut dyn FnMut(&mut dyn ArrayWriter) -> EncodeResult,
    ) -> EncodeResult {
        self.encode_structured_nested(ANY_VALUE_ARRAY_VALUE, |encoder| f(encoder))
    }
}

impl<B: BoundedBuf> MapWriter for NestedEncoder<'_, B> {
    fn encode_entry(
        &mut self,
        key: &str,
        f: &mut dyn FnMut(&mut dyn ValueWriter) -> EncodeResult,
    ) -> EncodeResult {
        let depth = self.depth;
        self.member(MAP_CUTOFF, |buf| {
            bounded::key_value_encoder(buf, KEY_VALUE_LIST_VALUES, key, depth, &mut |encoder| {
                encoder.encode_structured(&mut *f)
            })
        })
    }
}

impl<B: BoundedBuf> ArrayWriter for NestedEncoder<'_, B> {
    fn encode_element(
        &mut self,
        f: &mut dyn FnMut(&mut dyn ValueWriter) -> EncodeResult,
    ) -> EncodeResult {
        let depth = self.depth;
        self.member(ARRAY_CUTOFF, |buf| {
            bounded::value_encoder(buf, ARRAY_VALUE_VALUES, depth, &mut |encoder| {
                encoder.encode_structured(&mut *f)
            })
        })
    }
}

// The main tracing visitor
impl<B: BoundedBuf> tracing::field::Visit for DirectFieldVisitor<'_, B> {
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.record_toplevel_field(field, |buf| {
            bounded::primtiive_field(buf, ANY_VALUE_DOUBLE_VALUE, wire_types::FIXED64, |buf| {
                buf.try_extend_slice(&value.to_le_bytes())
            })
            .map(|t| t.into())
        });
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.record_toplevel_field(field, |buf| {
            bounded::primtiive_field(buf, ANY_VALUE_INT_VALUE, wire_types::VARINT, |buf| {
                buf.encode_varint(value as u64)
            })
            .map(|t| t.into())
        });
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.record_i64(field, value as i64);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.record_toplevel_field(field, |buf| {
            bounded::primtiive_field(buf, ANY_VALUE_BOOL_VALUE, wire_types::VARINT, |buf| {
                buf.encode_varint(u64::from(value))
            })
            .map(|t| t.into())
        })
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        // Preserve native tracing's empty-message convention, not slogr's required body.
        if !self.body_first
            && value.is_empty()
            && let LogFieldLocation::Body = LogFieldLocation::new(field, false)
        {
            return;
        }
        self.record_toplevel_field(field, |buf| {
            bounded::string_truncating(buf, ANY_VALUE_STRING_VALUE, value)
        })
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.record_toplevel_field(field, |buf| bounded::debug_string(buf, value))
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        match value.downcast_ref::<StructCarrier>() {
            Some(structured) => self.record_structured(field, structured.value()),
            None => self.record_debug(field, &format_args!("{value}")),
        }
    }
}

/// Convert tracing Level to OTLP severity number.
///
/// See: <https://opentelemetry.io/docs/specs/otel/logs/data-model/#field-severitynumber>
#[inline]
#[must_use]
pub const fn level_to_severity_number(level: &Level) -> u8 {
    match *level {
        Level::TRACE => 1,
        Level::DEBUG => 5,
        Level::INFO => 9,
        Level::WARN => 13,
        Level::ERROR => 17,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StructBox;
    use crate::buffer::{ProtoBuffer, StackProtoBuffer};
    use crate::test_proto::any_value::{Value as AnyValueKind, Value::StringValue};
    use crate::test_proto::{AnyValue, KeyValue, KeyValue as ProtoKeyValue, LogRecord};
    use prost::Message;
    use tracing::field::Visit;

    /// Scenario: one value callback writes two AnyValue variants.
    /// Guarantees: debug builds reject ambiguous oneof encodings.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "value callback must write at most one AnyValue variant")]
    fn value_encoder_rejects_multiple_variants() {
        let mut buf = ProtoBuffer::with_capacity(32);
        let mut write = |writer: &mut ValueEncoder<'_, ProtoBuffer>| -> TruncateResult {
            let _ = writer.encode_i64(1);
            let _ = writer.encode_bool(true);
            Ok(Truncation::None)
        };

        let _ = bounded::value_encoder(&mut buf, LOG_RECORD_BODY, 0, &mut write);
    }

    /// Scenario: four large string attributes compete for a 256-byte buffer.
    /// Guarantees: each uses at most half the remaining budget, is truncated,
    /// and increments `dropped_attributes_count`.
    #[test]
    fn record_str_halves_remaining_budget_across_attributes() {
        const INLINE: usize = 256;
        let big = "x".repeat(1000);

        let meta = &BUDGET_TEST_METADATA;
        let fields = meta.fields();
        let a = fields.field("a").unwrap();
        let b = fields.field("b").unwrap();
        let c = fields.field("c").unwrap();
        let d = fields.field("d").unwrap();

        // Drive the visitor directly with each (field, value) pair, which is
        // what `event.record(visitor)` does internally.
        let mut buf = StackProtoBuffer::<INLINE>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_str(&a, big.as_str());
        visitor.record_str(&b, big.as_str());
        visitor.record_str(&c, big.as_str());
        visitor.record_str(&d, big.as_str());
        let dropped = visitor.dropped_count();

        // All four values should be truncated.
        assert_eq!(dropped, 4, "expected 4 truncated attributes");

        // Buffer should not exceed the inline limit.
        assert!(
            buf.len() <= INLINE,
            "buffer len {} exceeds INLINE {}",
            buf.len(),
            INLINE
        );

        // Decode each attribute as a KeyValue (field tag = LOG_RECORD_ATTRIBUTES = 6,
        // wire type LEN). Walk the buffer manually.
        let bytes = buf.as_ref().to_vec();
        let mut cursor = bytes.as_slice();
        let mut decoded: Vec<(String, String)> = Vec::new();
        while !cursor.is_empty() {
            let (tag, n) = read_varint(cursor);
            cursor = &cursor[n..];
            let field_num = tag >> 3;
            let wtype = tag & 0x7;
            assert_eq!(wtype, wire_types::LEN, "expected LEN wire type");
            assert_eq!(field_num, LOG_RECORD_ATTRIBUTES, "expected ATTRIBUTES tag");
            let (len, n) = read_varint(cursor);
            cursor = &cursor[n..];
            let len = len as usize;
            let kv_bytes = &cursor[..len];
            cursor = &cursor[len..];
            let kv = ProtoKeyValue::decode(kv_bytes).unwrap();
            let val = kv
                .value
                .as_ref()
                .and_then(|v| v.value.as_ref())
                .map(|v| match v {
                    StringValue(s) => s.clone(),
                    _ => panic!("expected string value"),
                })
                .unwrap();
            decoded.push((kv.key, val));
        }

        assert_eq!(decoded.len(), 4, "expected 4 decoded attributes");

        // Each value should end with the truncation suffix.
        let suffix = std::str::from_utf8(TRUNCATION_SUFFIX).unwrap();
        for (k, v) in &decoded {
            assert!(
                v.ends_with(suffix),
                "attribute {k} value should end with {suffix}, got len {}",
                v.len()
            );
        }

        // Each successive value should be smaller (halving budget). We compare
        // value lengths rather than exact sizes to avoid brittleness.
        let lens: Vec<usize> = decoded.iter().map(|(_, v)| v.len()).collect();
        assert!(
            lens.windows(2).all(|w| w[0] > w[1]),
            "expected strictly decreasing value lengths, got {lens:?}"
        );

        // Sanity-check the rough magnitudes from the user's spec
        // (~100 / ~50 / ~25 / ~10), accommodating overhead variation.
        // Observed: ~[118, 55, 23, 7].
        assert!(
            lens[0] > 60 && lens[0] < 140,
            "1st value len {} out of expected range",
            lens[0]
        );
        assert!(
            lens[1] > 25 && lens[1] < 80,
            "2nd value len {} out of expected range",
            lens[1]
        );
        assert!(
            lens[2] > 10 && lens[2] < 50,
            "3rd value len {} out of expected range",
            lens[2]
        );
        // Last value may be just the suffix (~5 bytes) up to ~25 bytes.
        assert!(
            lens[3] >= suffix.len() && lens[3] < 30,
            "4th value len {} out of expected range",
            lens[3]
        );
    }

    /// Scenario: a Debug value fits in the attribute budget.
    /// Guarantees: its direct encoding is decodable and matches `format!("{:?}")`.
    #[test]
    fn record_debug_writes_fmt_output_without_intermediate_string() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Payload {
            id: u32,
            label: &'static str,
        }
        let value = Payload {
            id: 7,
            label: "hello",
        };
        let expected_repr = format!("{:?}", value);

        let meta = &DEBUG_TEST_METADATA;
        let fields = meta.fields();
        let dbg = fields.field("dbg").unwrap();

        let mut buf = StackProtoBuffer::<256>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_debug(&dbg, &value);
        assert_eq!(visitor.dropped_count(), 0, "no fields should be dropped");

        // Decode the single KeyValue from the buffer.
        let bytes = buf.as_ref().to_vec();
        let mut cursor = bytes.as_slice();
        let (tag, n) = read_varint(cursor);
        cursor = &cursor[n..];
        assert_eq!(tag >> 3, LOG_RECORD_ATTRIBUTES);
        assert_eq!(tag & 0x7, wire_types::LEN);
        let (len, n) = read_varint(cursor);
        cursor = &cursor[n..];
        let kv = ProtoKeyValue::decode(&cursor[..len as usize]).unwrap();
        assert_eq!(kv.key, "dbg");
        let s = match kv.value.as_ref().unwrap().value.as_ref().unwrap() {
            StringValue(s) => s.clone(),
            other => panic!("expected StringValue, got {other:?}"),
        };
        assert_eq!(s, expected_repr);
    }

    /// Scenario: a Debug value overflows the available buffer.
    /// Guarantees: the value is truncated with a dagger suffix instead of
    /// being dropped entirely, `dropped_count` is incremented, and the
    /// encoded KeyValue is decodable with the suffix at the end.
    #[test]
    fn record_debug_overflow_truncates_with_suffix() {
        // A Debug impl that writes far more than the buffer can hold.
        struct Huge;
        impl std::fmt::Debug for Huge {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                // 4 KiB of output into a 64-byte buffer.
                for _ in 0..512 {
                    f.write_str("xxxxxxxx")?;
                }
                Ok(())
            }
        }

        let meta = &DEBUG_TEST_METADATA;
        let fields = meta.fields();
        let dbg = fields.field("dbg").unwrap();

        let mut buf = StackProtoBuffer::<64>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_debug(&dbg, &Huge);
        assert_eq!(
            visitor.dropped_count(),
            1,
            "the single oversized debug field should be counted as dropped (truncated)"
        );

        // Buffer should contain a truncated KeyValue, not be empty.
        assert!(
            buf.len() > 0,
            "buffer should contain a truncated attribute, not be empty"
        );

        // Decode the KeyValue and verify the value ends with the suffix.
        let bytes = buf.as_ref().to_vec();
        let mut cursor = bytes.as_slice();
        let (tag, n) = read_varint(cursor);
        cursor = &cursor[n..];
        assert_eq!(tag >> 3, LOG_RECORD_ATTRIBUTES);
        assert_eq!(tag & 0x7, wire_types::LEN);
        let (len, n) = read_varint(cursor);
        cursor = &cursor[n..];
        let kv = ProtoKeyValue::decode(&cursor[..len as usize]).unwrap();
        assert_eq!(kv.key, "dbg");
        let s = match kv.value.as_ref().unwrap().value.as_ref().unwrap() {
            StringValue(s) => s.clone(),
            other => panic!("expected StringValue, got {other:?}"),
        };
        let suffix = std::str::from_utf8(TRUNCATION_SUFFIX).unwrap();
        assert!(
            s.ends_with(suffix),
            "truncated debug value should end with {suffix}, got: {s}"
        );
        // The value should contain some content before the suffix.
        assert!(
            s.len() > suffix.len(),
            "truncated value should have content before the suffix"
        );
    }

    /// Scenario: a Debug value cannot fit even in truncated form because the
    /// buffer is too small for key + suffix + protobuf overhead.
    /// Guarantees: the attribute is hard-dropped (buffer rolled back to
    /// pre-call length) and `dropped_count` is incremented.
    #[test]
    fn record_debug_hard_drop_when_budget_too_small() {
        struct Tiny;
        impl std::fmt::Debug for Tiny {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("x")
            }
        }

        let meta = &DEBUG_TEST_METADATA;
        let fields = meta.fields();
        let dbg = fields.field("dbg").unwrap();

        // A buffer so tiny that even the key "dbg" + protobuf overhead
        // cannot fit, forcing a hard drop.
        let mut buf = StackProtoBuffer::<8>::default();
        let before_len = buf.len();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_debug(&dbg, &Tiny);
        assert_eq!(
            visitor.dropped_count(),
            1,
            "the field should be counted as dropped"
        );
        assert_eq!(
            buf.len(),
            before_len,
            "buffer must be rolled back to its pre-call length on hard drop"
        );
    }

    /// Scenario: an event has an empty string message and an empty string attribute.
    /// Guarantees: the message body is omitted while the empty attribute is preserved.
    #[test]
    fn record_str_omits_empty_message_but_preserves_empty_attribute() {
        let fields = tracing::field::FieldSet::new(
            &["message", "empty"],
            tracing::callsite::Identifier(&MESSAGE_TEST_CALLSITE),
        );
        let mut buf = StackProtoBuffer::<256>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_str(&fields.field("message").unwrap(), "");
        visitor.record_str(&fields.field("empty").unwrap(), "");
        let decoded = LogRecord::decode(buf.as_ref()).expect("valid log record");
        assert!(decoded.body.is_none());
        assert_eq!(
            decoded.attributes,
            [KeyValue::new("empty", AnyValue::new_string(""))]
        );
    }

    /// Scenario: formatting produces an empty message and attribute between other attributes.
    /// Guarantees: the empty string body and all attributes remain present without counted drops.
    #[test]
    fn record_debug_preserves_empty_message_and_attribute() {
        let fields = tracing::field::FieldSet::new(
            &["before", "message", "empty", "after"],
            tracing::callsite::Identifier(&MESSAGE_TEST_CALLSITE),
        );
        let mut buf = StackProtoBuffer::<256>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_i64(&fields.field("before").unwrap(), 1);
        visitor.record_debug(&fields.field("message").unwrap(), &format_args!("{}", ""));
        visitor.record_debug(&fields.field("empty").unwrap(), &format_args!(""));
        visitor.record_i64(&fields.field("after").unwrap(), 2);
        assert_eq!(visitor.dropped_count(), 0);
        let decoded = LogRecord::decode(buf.as_ref()).expect("valid log record");
        assert_eq!(decoded.body, Some(AnyValue::new_string("")));
        assert_eq!(
            decoded.attributes,
            [
                KeyValue::new("before", AnyValue::new_int(1)),
                KeyValue::new("empty", AnyValue::new_string("")),
                KeyValue::new("after", AnyValue::new_int(2)),
            ]
        );
    }

    /// Scenario: a bare `&str` `message` body overflows the available buffer.
    /// Guarantees: the decoded body ends with a dagger suffix without incrementing
    /// OTLP `dropped_attributes_count`.
    #[test]
    fn record_str_message_overflow_truncates_with_suffix() {
        let meta = &MESSAGE_TEST_METADATA;
        let fields = meta.fields();
        let message = fields.field("message").unwrap();

        // 4 KiB of content into a 64-byte buffer.
        let huge = "x".repeat(4096);

        let mut buf = StackProtoBuffer::<64>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_str(&message, &huge);
        assert_eq!(
            visitor.dropped_count(),
            0,
            "body truncation must not be counted as a dropped attribute"
        );

        assert!(
            buf.len() > 0,
            "buffer should contain a truncated body, not be empty"
        );

        let record = LogRecord::decode(buf.as_ref()).expect("log record should decode");
        assert_eq!(record.dropped_attributes_count, 0);
        let s = match record.body.unwrap().value.unwrap() {
            StringValue(s) => s,
            other => panic!("expected StringValue, got {other:?}"),
        };
        let suffix = std::str::from_utf8(TRUNCATION_SUFFIX).unwrap();
        assert!(
            s.ends_with(suffix),
            "truncated body should end with {suffix}, got: {s}"
        );
        assert!(
            s.len() > suffix.len(),
            "truncated body should have content before the suffix"
        );
    }

    /// Scenario: a Debug-formatted `message` body overflows the available buffer.
    /// Guarantees: the decoded body ends with a dagger suffix without incrementing
    /// OTLP `dropped_attributes_count`.
    #[test]
    fn record_debug_message_overflow_truncates_with_suffix() {
        struct Huge;
        impl std::fmt::Debug for Huge {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                for _ in 0..512 {
                    f.write_str("xxxxxxxx")?;
                }
                Ok(())
            }
        }

        let fields = MESSAGE_TEST_METADATA.fields();
        let message = fields.field("message").unwrap();
        let mut buf = StackProtoBuffer::<64>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_debug(&message, &Huge);
        assert_eq!(
            visitor.dropped_count(),
            0,
            "body truncation must not be counted as a dropped attribute"
        );

        let record = LogRecord::decode(buf.as_ref()).expect("log record should decode");
        assert_eq!(record.dropped_attributes_count, 0);
        let s = match record.body.unwrap().value.unwrap() {
            StringValue(s) => s,
            other => panic!("expected StringValue, got {other:?}"),
        };
        let suffix = std::str::from_utf8(TRUNCATION_SUFFIX).unwrap();
        assert!(
            s.ends_with(suffix),
            "truncated body should end with {suffix}, got: {s}"
        );
        assert!(
            s.len() > suffix.len(),
            "truncated body should have content before the suffix"
        );
    }

    static MESSAGE_TEST_CALLSITE: MessageTestCallsite = MessageTestCallsite;

    struct MessageTestCallsite;

    impl tracing::Callsite for MessageTestCallsite {
        fn set_interest(&self, _: tracing::subscriber::Interest) {}
        fn metadata(&self) -> &tracing::Metadata<'_> {
            &MESSAGE_TEST_METADATA
        }
    }

    static MESSAGE_TEST_METADATA: tracing::Metadata<'static> = tracing::Metadata::new(
        "message_test",
        "slogr",
        Level::INFO,
        Some(file!()),
        Some(line!()),
        Some(module_path!()),
        tracing::field::FieldSet::new(
            &["message"],
            tracing::callsite::Identifier(&MESSAGE_TEST_CALLSITE),
        ),
        tracing::metadata::Kind::EVENT,
    );

    /// Scenario: four Debug values compete for a 256-byte buffer with the
    /// halving budget policy.
    /// Guarantees: all four values are truncated (each ends with a dagger suffix),
    /// each successive value is no longer than the previous, all four
    /// increment the dropped count, and the buffer stays within the inline limit.
    #[test]
    fn record_debug_halves_remaining_budget_across_attributes() {
        const INLINE: usize = 256;

        // A Debug impl that writes 1000 bytes.
        struct Big;
        impl std::fmt::Debug for Big {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                for _ in 0..125 {
                    f.write_str("xxxxxxxx")?;
                }
                Ok(())
            }
        }

        let meta = &BUDGET_TEST_METADATA;
        let fields = meta.fields();
        let a = fields.field("a").unwrap();
        let b = fields.field("b").unwrap();
        let c = fields.field("c").unwrap();
        let d = fields.field("d").unwrap();

        let mut buf = StackProtoBuffer::<INLINE>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_debug(&a, &Big);
        visitor.record_debug(&b, &Big);
        visitor.record_debug(&c, &Big);
        visitor.record_debug(&d, &Big);
        let dropped = visitor.dropped_count();

        assert_eq!(dropped, 4, "expected 4 truncated attributes");
        assert!(
            buf.len() <= INLINE,
            "buffer len {} exceeds INLINE {}",
            buf.len(),
            INLINE
        );

        // Decode each attribute.
        let bytes = buf.as_ref().to_vec();
        let mut cursor = bytes.as_slice();
        let mut decoded: Vec<(String, String)> = Vec::new();
        while !cursor.is_empty() {
            let (tag, n) = read_varint(cursor);
            cursor = &cursor[n..];
            let field_num = tag >> 3;
            let wtype = tag & 0x7;
            assert_eq!(wtype, wire_types::LEN, "expected LEN wire type");
            assert_eq!(field_num, LOG_RECORD_ATTRIBUTES, "expected ATTRIBUTES tag");
            let (len, n) = read_varint(cursor);
            cursor = &cursor[n..];
            let len = len as usize;
            let kv_bytes = &cursor[..len];
            cursor = &cursor[len..];
            let kv = ProtoKeyValue::decode(kv_bytes).unwrap();
            let val = kv
                .value
                .as_ref()
                .and_then(|v| v.value.as_ref())
                .map(|v| match v {
                    StringValue(s) => s.clone(),
                    _ => panic!("expected string value"),
                })
                .unwrap();
            decoded.push((kv.key, val));
        }

        assert_eq!(decoded.len(), 4, "expected 4 decoded attributes");

        let suffix = std::str::from_utf8(TRUNCATION_SUFFIX).unwrap();
        for (k, v) in &decoded {
            assert!(
                v.ends_with(suffix),
                "attribute {k} value should end with {suffix}, got len {}",
                v.len()
            );
        }

        // Each successive value should be no longer than the previous one
        // as the remaining budget is halved across attributes.
        let lens: Vec<usize> = decoded.iter().map(|(_, v)| v.len()).collect();
        assert!(
            lens.windows(2).all(|w| w[0] >= w[1]),
            "expected non-increasing value lengths, got {lens:?}"
        );
    }

    static DEBUG_TEST_CALLSITE: DebugTestCallsite = DebugTestCallsite;

    struct DebugTestCallsite;

    impl tracing::Callsite for DebugTestCallsite {
        fn set_interest(&self, _: tracing::subscriber::Interest) {}
        fn metadata(&self) -> &tracing::Metadata<'_> {
            &DEBUG_TEST_METADATA
        }
    }

    static DEBUG_TEST_METADATA: tracing::Metadata<'static> = tracing::Metadata::new(
        "debug_test",
        "slogr",
        Level::INFO,
        Some(file!()),
        Some(line!()),
        Some(module_path!()),
        tracing::field::FieldSet::new(
            &["dbg"],
            tracing::callsite::Identifier(&DEBUG_TEST_CALLSITE),
        ),
        tracing::metadata::Kind::EVENT,
    );

    /// Read an unsigned varint, returning (value, bytes_consumed).
    fn read_varint(bytes: &[u8]) -> (u64, usize) {
        let mut value = 0u64;
        let mut shift = 0;
        for (i, &b) in bytes.iter().enumerate() {
            value |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return (value, i + 1);
            }
            shift += 7;
        }
        panic!("truncated varint");
    }

    /// Scenario: raw AnyValues of every wire variant are copied at a depth beyond the callback limit.
    /// Guarantees: heap and stack buffers preserve exact bytes, including non-minimal encodings, and a one-byte-short budget writes nothing.
    #[test]
    fn raw_any_values_copy_atomically_without_depth_checks() {
        fn check<B: BoundedBuf>(buf: &mut B, encoded: &[u8]) {
            for limit in [encoded.len() - 1, encoded.len()] {
                buf.clear();
                let result = buf.with_max_remaining(limit, |buf| {
                    ValueEncoder::new(buf, MAX_DEPTH + 1)
                        .encode_structured(|w| w.encode_raw_any_value(encoded))
                });
                if limit == encoded.len() {
                    assert_eq!(result, Ok(Truncation::None));
                    assert_eq!(buf.as_slice(), encoded);
                } else {
                    assert_eq!(result, Err(EncodeFailure::Dropped));
                    assert!(buf.is_empty());
                }
            }
        }

        let mut nested = AnyValue::new_bool(true);
        for _ in 0..=MAX_DEPTH {
            nested = AnyValue::new_array(vec![nested]);
        }
        let values = [
            AnyValue::new_string("payload"),
            AnyValue::new_bool(true),
            AnyValue::new_int(-1),
            AnyValue {
                value: Some(AnyValueKind::DoubleValue(3.5)),
            },
            AnyValue {
                value: Some(AnyValueKind::BytesValue(vec![0, 255])),
            },
            nested,
            AnyValue::new_kvlist(vec![KeyValue::new("key", AnyValue::new_int(7))]),
        ];
        let mut encoded = values
            .iter()
            .map(Message::encode_to_vec)
            .collect::<Vec<_>>();
        encoded.push(vec![0x18, 0x81, 0x00]);
        let mut heap = ProtoBuffer::with_capacity(16);
        let mut stack = StackProtoBuffer::<512>::default();
        for value in encoded {
            check(&mut heap, &value);
            check(&mut stack, &value);
        }
    }

    /// Scenario: a raw value exceeds its map-entry budget after a completed member.
    /// Guarantees: the raw value is omitted atomically, the prefix gets one dagger, later top-level fields survive, and loss is counted once.
    #[test]
    fn raw_any_value_failure_uses_container_cutoff() {
        struct RawValue(Vec<u8>);
        impl StructuredValue for RawValue {
            fn encode(&self, w: &mut dyn ValueWriter) -> EncodeResult {
                w.encode_map(&mut |m| {
                    m.encode_entry("first", &mut |v| v.encode_bool(true))?;
                    m.encode_entry("raw", &mut |v| v.encode_raw_any_value(&self.0))?;
                    panic!("oversized raw value must stop the container")
                })
            }
        }
        let value = StructBox::new(RawValue(
            AnyValue::new_string("x".repeat(1024)).encode_to_vec(),
        ));
        let fields = BUDGET_TEST_METADATA.fields();
        let a = fields.field("a").expect("known field");
        let b = fields.field("b").expect("known field");
        let mut buf = StackProtoBuffer::<256>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_error(&a, value.as_struct());
        visitor.record_bool(&b, true);
        assert_eq!(visitor.dropped_count(), 1);
        let record = LogRecord::decode(buf.as_ref()).expect("valid marked prefix");
        assert_eq!(
            record.attributes,
            vec![
                KeyValue::new(
                    "a",
                    AnyValue::new_kvlist(vec![
                        KeyValue::new("first", AnyValue::new_bool(true)),
                        KeyValue::new("[\u{2020}]", AnyValue::new_string("[\u{2020}]")),
                    ]),
                ),
                KeyValue::new("b", AnyValue::new_bool(true)),
            ]
        );
    }

    /// Scenario: callers supply empty, unknown, wrongly framed, or multiple-field raw enclosures.
    /// Guarantees: debug builds reject invalid AnyValue enclosures before copying their bytes.
    #[cfg(debug_assertions)]
    #[test]
    fn raw_any_value_rejects_invalid_enclosures() {
        let invalid: &[&[u8]] = &[
            &[],
            &[0x40, 1],
            &[0x08, 1],
            &[0x0a, 2, b'x'],
            &[0x21, 0],
            &[0x18, 0x80],
            &[0x10, 1, 0x18, 1],
            &[0x10, 1, 0x10, 0],
            &[0x0a, 0, 0],
        ];
        for encoded in invalid {
            let panic = std::panic::catch_unwind(|| {
                let mut buf = StackProtoBuffer::<32>::default();
                let _ = ValueEncoder::new(&mut buf, 0)
                    .encode_structured(|w| w.encode_raw_any_value(encoded));
            })
            .expect_err("invalid enclosure must panic");
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .expect("panic message");
            assert!(message.contains("raw encoding must contain exactly one recognized AnyValue"));
        }
    }

    /// Scenario: a callback writes a raw AnyValue and then attempts another value.
    /// Guarantees: raw copying participates in the same debug-only single-value guard as typed encoding.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "value callback must write at most one AnyValue variant")]
    fn raw_any_value_enforces_single_value() {
        let mut buf = StackProtoBuffer::<32>::default();
        let _ = ValueEncoder::new(&mut buf, 0).encode_structured(|w| {
            w.encode_raw_any_value(&[0x10, 1])?;
            w.encode_i64(7)
        });
    }

    struct Probe {
        id: i64,
        name: String,
        tags: Vec<String>,
    }

    impl StructuredValue for Probe {
        fn encode(&self, w: &mut dyn ValueWriter) -> EncodeResult {
            w.encode_map(&mut |m| {
                m.encode_entry("id", &mut |v| v.encode_i64(self.id))?;
                m.encode_entry("name", &mut |v| v.encode_str(&self.name))?;
                m.encode_entry("tags", &mut |v| {
                    v.encode_array(&mut |a| {
                        for tag in &self.tags {
                            a.encode_element(&mut |v| v.encode_str(tag))?;
                        }
                        Ok(())
                    })
                })
            })
        }
    }

    /// Scenario: a boxed map/array value is mutated between events and encoded under small and roomy budgets.
    /// Guarantees: logging borrows current state, preserves wire types across all placeholder widths, stays within each budget, and counts partial values once.
    #[test]
    fn boxed_values_preserve_mutation_and_budgets() {
        let mut slot = StructBox::new(Probe {
            id: 0,
            name: "example".into(),
            tags: vec!["a".into(), "b".into()],
        });
        let field = BUDGET_TEST_METADATA
            .fields()
            .field("a")
            .expect("known field");
        for count in [1, 2] {
            slot.id = count;
            let mut buf = ProtoBuffer::with_capacity_and_limit(16384, 16384);
            let mut visitor = DirectFieldVisitor::new(&mut buf);
            visitor.record_error(&field, slot.as_struct());
            assert_eq!(visitor.dropped_count(), 0);
            let expected = LogRecord::decode(buf.as_ref()).expect("valid record");
            let Some(AnyValueKind::KvlistValue(map)) =
                &expected.attributes[0].value.as_ref().expect("value").value
            else {
                panic!("expected map");
            };
            assert_eq!(map.values[0], KeyValue::new("id", AnyValue::new_int(count)));
            for limit in [8, 32, 64, 128, 2048, 1 << 16, 1 << 28] {
                let mut buf = ProtoBuffer::with_capacity_and_limit(limit.min(256), limit);
                let mut visitor = DirectFieldVisitor::new(&mut buf);
                visitor.record_error(&field, slot.as_struct());
                assert_eq!(
                    visitor.dropped_count(),
                    u32::from(limit <= 128),
                    "limit {limit}"
                );
                assert!(buf.len() <= limit);
                let decoded = LogRecord::decode(buf.as_ref()).expect("valid bounded record");
                if limit >= 2048 {
                    assert_eq!(decoded, expected);
                }
            }
        }
    }

    /// Scenario: a native tracing event supplies a structured `message` with string, integer, and array fields.
    /// Guarantees: the body contains only caller-supplied typed fields, including an ordinary field named name.
    #[test]
    fn structured_message_is_log_body() {
        let slot = StructBox::new(Probe {
            id: 7,
            name: "example".into(),
            tags: vec!["a".into(), "b".into()],
        });
        let mut buf = StackProtoBuffer::<256>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_error(
            &MESSAGE_TEST_METADATA.fields().field("message").unwrap(),
            slot.as_struct(),
        );
        let decoded = LogRecord::decode(buf.as_ref()).expect("valid record");

        assert!(decoded.attributes.is_empty());
        assert_eq!(
            decoded.body,
            Some(AnyValue::new_kvlist(vec![
                KeyValue::new("id", AnyValue::new_int(7)),
                KeyValue::new("name", AnyValue::new_string("example")),
                KeyValue::new(
                    "tags",
                    AnyValue::new_array(vec![AnyValue::new_string("a"), AnyValue::new_string("b")]),
                ),
            ]))
        );
    }

    /// Scenario: an owned structured root encodes a temporary nested view borrowing its strings.
    /// Guarantees: non-static structured views encode borrowed strings through the stored carrier.
    #[test]
    fn structured_values_encode_borrowed_nested_views() {
        struct State {
            name: String,
        }

        struct Snapshot<'a> {
            name: &'a str,
        }

        impl StructuredValue for Snapshot<'_> {
            fn encode(&self, w: &mut dyn ValueWriter) -> EncodeResult {
                w.encode_map(&mut |m| m.encode_entry("name", &mut |v| v.encode_str(self.name)))
            }
        }

        impl StructuredValue for State {
            fn encode(&self, w: &mut dyn ValueWriter) -> EncodeResult {
                let snapshot = Snapshot { name: &self.name };
                w.encode_map(&mut |m| m.encode_entry("snapshot", &mut |v| snapshot.encode(v)))
            }
        }

        let state = StructBox::new(State {
            name: "example".into(),
        });
        let field = BUDGET_TEST_METADATA
            .fields()
            .field("a")
            .expect("known field");
        let mut buf = ProtoBuffer::with_capacity_and_limit(16384, 16384);
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_error(&field, state.as_struct());
        assert_eq!(visitor.dropped_count(), 0);
        let decoded = LogRecord::decode(buf.as_ref()).expect("valid record");
        assert_eq!(
            decoded.attributes,
            vec![KeyValue::new(
                "a",
                AnyValue::new_kvlist(vec![KeyValue::new(
                    "snapshot",
                    AnyValue::new_kvlist(vec![KeyValue::new(
                        "name",
                        AnyValue::new_string("example"),
                    )]),
                )]),
            )],
        );
    }

    /// Scenario: a map entry writes a scalar and then fails, exiting its callback through `?`.
    /// Guarantees: the field rolls back, body and attribute prefixes end with a cutoff entry, and later log fields survive.
    #[test]
    fn structured_map_retains_prefix_when_entry_fails() {
        struct Partial {
            attempts: std::cell::Cell<usize>,
        }
        impl StructuredValue for Partial {
            fn encode(&self, w: &mut dyn ValueWriter) -> EncodeResult {
                w.encode_map(&mut |m| {
                    m.encode_entry("first", &mut |v| v.encode_i64(7))?;
                    m.encode_entry("failed", &mut |v| {
                        self.attempts.set(self.attempts.get() + 1);
                        v.encode_bool(true)?;
                        Err(EncodeFailure::Dropped)
                    })?;
                    panic!("failed entry must stop the callback")
                })
            }
        }
        let value = StructBox::new(Partial {
            attempts: std::cell::Cell::new(0),
        });
        let message = MESSAGE_TEST_METADATA
            .fields()
            .field("message")
            .expect("known field");
        let fields = BUDGET_TEST_METADATA.fields();
        let a = fields.field("a").expect("known field");
        let b = fields.field("b").expect("known field");
        let mut buf = ProtoBuffer::with_capacity_and_limit(256, 2048);
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_error(&message, value.as_struct());
        visitor.record_error(&a, value.as_struct());
        visitor.record_bool(&b, true);
        assert_eq!(visitor.dropped_count(), 1);
        assert_eq!(value.attempts.get(), 2);
        let prefix = AnyValue::new_kvlist(vec![
            KeyValue::new("first", AnyValue::new_int(7)),
            KeyValue::new("[\u{2020}]", AnyValue::new_string("[\u{2020}]")),
        ]);
        let record = LogRecord::decode(buf.as_ref()).expect("valid prefix framing");
        assert_eq!(record.body, Some(prefix.clone()));
        assert_eq!(
            record.attributes,
            vec![
                KeyValue::new("a", prefix),
                KeyValue::new("b", AnyValue::new_bool(true)),
            ]
        );
    }

    /// Scenario: a map callback succeeds empty or populated, fails inside, or succeeds before an outer failure.
    /// Guarantees: complete maps have no marker, callback errors mark retained prefixes, and outer errors roll back the value without losing later fields.
    #[test]
    fn structured_map_finalizes_at_callback_boundary() {
        let fields = BUDGET_TEST_METADATA.fields();
        let a = fields.field("a").expect("known field");
        let b = fields.field("b").expect("known field");
        for completion in ["empty", "complete", "inside_callback", "outside_callback"] {
            let mut buf = ProtoBuffer::with_capacity_and_limit(256, 2048);
            let mut visitor = DirectFieldVisitor::new(&mut buf);
            visitor.record_toplevel_field(&a, |buf| {
                ValueEncoder::new(buf, 0).encode_structured(|w| {
                    w.encode_map(&mut |m| {
                        if completion == "empty" {
                            return Ok(());
                        }
                        m.encode_entry("first", &mut |v| v.encode_i64(7))?;
                        if completion == "inside_callback" {
                            return Err(EncodeFailure::Dropped);
                        }
                        Ok(())
                    })?;
                    if completion != "outside_callback" {
                        return Ok(());
                    }
                    Err(EncodeFailure::Dropped)
                })
            });
            visitor.record_bool(&b, true);
            assert_eq!(
                visitor.dropped_count(),
                u32::from(matches!(completion, "inside_callback" | "outside_callback")),
                "{completion}",
            );
            let mut expected = Vec::new();
            if completion != "outside_callback" {
                expected.push(KeyValue::new(
                    "a",
                    AnyValue::new_kvlist(if completion == "empty" {
                        vec![]
                    } else if completion == "inside_callback" {
                        vec![
                            KeyValue::new("first", AnyValue::new_int(7)),
                            KeyValue::new("[\u{2020}]", AnyValue::new_string("[\u{2020}]")),
                        ]
                    } else {
                        vec![KeyValue::new("first", AnyValue::new_int(7))]
                    }),
                ));
            }
            expected.push(KeyValue::new("b", AnyValue::new_bool(true)));
            let record = LogRecord::decode(buf.as_ref()).expect("valid finalized record");
            assert_eq!(record.attributes, expected, "{completion}");
        }
    }

    /// Scenario: a structured writer returns failure after writing a valid scalar body or attribute.
    /// Guarantees: the returned failure rolls back the value, counts only the attribute loss, and preserves later fields.
    #[test]
    fn structured_writer_failure_rolls_back_body_and_attribute() {
        struct FailAfterWrite;

        impl StructuredValue for FailAfterWrite {
            fn encode(&self, w: &mut dyn ValueWriter) -> EncodeResult {
                w.encode_i64(7)?;
                Err(EncodeFailure::Dropped)
            }
        }

        let slot = StructBox::new(FailAfterWrite);
        let fields = tracing::field::FieldSet::new(
            &["failed", "message", "tail"],
            tracing::callsite::Identifier(&MESSAGE_TEST_CALLSITE),
        );
        let mut buf = StackProtoBuffer::<256>::default();
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_error(&fields.field("failed").unwrap(), slot.as_struct());
        visitor.record_error(&fields.field("message").unwrap(), slot.as_struct());
        visitor.record_bool(&fields.field("tail").unwrap(), true);
        assert_eq!(visitor.dropped_count(), 1);
        let decoded = LogRecord::decode(buf.as_ref()).expect("valid record");
        assert!(decoded.body.is_none());
        assert_eq!(
            decoded.attributes,
            vec![KeyValue::new("tail", AnyValue::new_bool(true))]
        );
    }

    /// Scenario: a map key or array element cannot fit although a later small member would fit.
    /// Guarantees: both containers mark retained prefixes, stop iterator advancement on failure, count loss once, and allow the next top-level attribute.
    #[test]
    fn containers_retain_prefix_after_hard_drop() {
        let fields = BUDGET_TEST_METADATA.fields();
        let a = fields.field("a").expect("known field");
        let b = fields.field("b").expect("known field");
        for (array, limit) in [(false, 128), (true, 64)] {
            let mut buf = ProtoBuffer::with_capacity_and_limit(limit, limit);
            let mut visitor = DirectFieldVisitor::new(&mut buf);
            let mut advances = 0;
            visitor.record_toplevel_field(&a, |buf| {
                ValueEncoder::new(buf, 0).encode_structured(|w| {
                    if array {
                        w.encode_array(&mut |a| {
                            for index in (0..3).inspect(|_| advances += 1) {
                                a.encode_element(&mut |v| match index {
                                    0 => v.encode_bool(true),
                                    1 => v.encode_i64(-1),
                                    _ => panic!("array stopped before this element"),
                                })?;
                            }
                            Ok(())
                        })
                    } else {
                        w.encode_map(&mut |m| {
                            let oversized = "x".repeat(256);
                            for key in ["first", &oversized, "last"]
                                .into_iter()
                                .inspect(|_| advances += 1)
                            {
                                m.encode_entry(key, &mut |v| {
                                    assert_eq!(key, "first", "failed key skips its value");
                                    v.encode_bool(true)
                                })?;
                            }
                            Ok(())
                        })
                    }
                })
            });
            assert_eq!(advances, 2);
            visitor.record_bool(&b, true);
            assert_eq!(visitor.dropped_count(), 1);
            assert!(buf.len() <= limit);
            let record = LogRecord::decode(buf.as_ref()).expect("valid record");
            let expected = if array {
                AnyValue::new_array(vec![
                    AnyValue::new_bool(true),
                    AnyValue::new_string("[\u{2020}]"),
                ])
            } else {
                AnyValue::new_kvlist(vec![
                    KeyValue::new("first", AnyValue::new_bool(true)),
                    KeyValue::new("[\u{2020}]", AnyValue::new_string("[\u{2020}]")),
                ])
            };
            assert_eq!(
                record.attributes,
                vec![
                    KeyValue::new("a", expected),
                    KeyValue::new("b", AnyValue::new_bool(true)),
                ]
            );
        }
    }

    /// Scenario: a map entry or array element writes bytes before its callback returns failure.
    /// Guarantees: the failed member rolls back, `?` stops later callbacks, and the marked prefix and next attribute survive with one loss count.
    #[test]
    fn member_callback_failure_retains_only_completed_prefix() {
        let fields = BUDGET_TEST_METADATA.fields();
        let a = fields.field("a").expect("known field");
        let b = fields.field("b").expect("known field");
        for array in [false, true] {
            let mut buf = ProtoBuffer::with_capacity_and_limit(256, 256);
            let mut visitor = DirectFieldVisitor::new(&mut buf);
            visitor.record_toplevel_field(&a, |buf| {
                ValueEncoder::new(buf, 0).encode_structured(|w| {
                    let mut fail = |v: &mut dyn ValueWriter| {
                        v.encode_i64(7)?;
                        Err(EncodeFailure::Dropped)
                    };
                    if array {
                        w.encode_array(&mut |a| {
                            a.encode_element(&mut |v| v.encode_bool(true))?;
                            a.encode_element(&mut fail)?;
                            panic!("failed element must stop the callback");
                        })
                    } else {
                        w.encode_map(&mut |m| {
                            m.encode_entry("first", &mut |v| v.encode_bool(true))?;
                            m.encode_entry("failed", &mut fail)?;
                            panic!("failed entry must stop the callback");
                        })
                    }
                })
            });
            visitor.record_bool(&b, true);
            assert_eq!(visitor.dropped_count(), 1);
            assert!(buf.len() <= 256);
            let record = LogRecord::decode(buf.as_ref()).expect("valid record");
            let expected = if array {
                AnyValue::new_array(vec![
                    AnyValue::new_bool(true),
                    AnyValue::new_string("[\u{2020}]"),
                ])
            } else {
                AnyValue::new_kvlist(vec![
                    KeyValue::new("first", AnyValue::new_bool(true)),
                    KeyValue::new("[\u{2020}]", AnyValue::new_string("[\u{2020}]")),
                ])
            };
            assert_eq!(
                record.attributes,
                vec![
                    KeyValue::new("a", expected),
                    KeyValue::new("b", AnyValue::new_bool(true)),
                ]
            );
        }
    }

    /// Scenario: a map or array string is truncated, leaving space for another member.
    /// Guarantees: partial UTF-8 strings, later members, and the next attribute survive, with one loss count for the containing attribute.
    #[test]
    fn partial_members_do_not_stop_containers() {
        let large = "\u{1f642}".repeat(1000);
        let fields = BUDGET_TEST_METADATA.fields();
        let a = fields.field("a").expect("known field");
        let b = fields.field("b").expect("known field");
        for array in [false, true] {
            let mut buf = ProtoBuffer::with_capacity_and_limit(256, 256);
            let mut visitor = DirectFieldVisitor::new(&mut buf);
            visitor.record_toplevel_field(&a, |buf| {
                ValueEncoder::new(buf, 0).encode_structured(|w| {
                    if array {
                        w.encode_array(&mut |a| {
                            a.encode_element(&mut |v| v.encode_str(&large))?;
                            a.encode_element(&mut |v| v.encode_bool(true))
                        })
                    } else {
                        w.encode_map(&mut |m| {
                            m.encode_entry("first", &mut |v| v.encode_str(&large))?;
                            m.encode_entry("last", &mut |v| v.encode_bool(true))
                        })
                    }
                })
            });
            visitor.record_bool(&b, true);
            assert_eq!(visitor.dropped_count(), 1);
            assert!(buf.len() <= 256);
            let mut attributes = LogRecord::decode(buf.as_ref())
                .expect("valid record")
                .attributes;
            assert_eq!(attributes.len(), 2);
            assert_eq!(
                attributes.pop(),
                Some(KeyValue::new("b", AnyValue::new_bool(true)))
            );
            let first = attributes.pop().expect("partial attribute");
            assert_eq!(first.key, "a");
            let value = first.value.expect("partial value");
            let members = match value.value.expect("container") {
                AnyValueKind::ArrayValue(a) => a.values,
                AnyValueKind::KvlistValue(m) => m
                    .values
                    .into_iter()
                    .map(|kv| kv.value.expect("value"))
                    .collect::<Vec<_>>(),
                _ => panic!("expected container"),
            };
            assert_eq!(members.len(), 2);
            let Some(StringValue(first)) = &members[0].value else {
                panic!("expected string");
            };
            assert!(first.ends_with("[\u{2020}]"));
            assert_ne!(first, &large);
            assert_eq!(members[1], AnyValue::new_bool(true));
        }
    }

    /// Scenario: an empty or failed container has a byte budget around its framing and cutoff-marker sizes.
    /// Guarantees: exact-fit markers are complete, insufficient budgets roll back, empty containers need no marker space, and later fields remain encodable.
    #[test]
    fn structured_cutoff_markers_fit_exactly() {
        for (array, marker_limit) in [(true, 13), (false, 22)] {
            for limit in [0, 3, 4, marker_limit - 1, marker_limit, marker_limit + 1] {
                for complete in [false, true] {
                    let mut buf = StackProtoBuffer::<64>::default();
                    let result = buf.with_max_remaining(limit, |buf| {
                        buf.try_atomic(|buf| {
                            bounded::value_encoder(buf, LOG_RECORD_BODY, 0, &mut |encoder| {
                                encoder.encode_structured(|w| {
                                    let result = if complete {
                                        Ok(())
                                    } else {
                                        Err(EncodeFailure::Dropped)
                                    };
                                    if array {
                                        w.encode_array(&mut |_| result)
                                    } else {
                                        w.encode_map(&mut |_| result)
                                    }
                                })
                            })
                        })
                    });
                    let fits = limit >= if complete { 4 } else { marker_limit };
                    assert_eq!(
                        result,
                        if fits {
                            Ok(if complete {
                                Truncation::None
                            } else {
                                Truncation::Some
                            })
                        } else {
                            Err(EncodeFailure::Dropped)
                        },
                        "array={array}, limit={limit}, complete={complete}",
                    );
                    assert!(buf.len() <= limit);
                    let field = BUDGET_TEST_METADATA
                        .fields()
                        .field("b")
                        .expect("known field");
                    DirectFieldVisitor::new(&mut buf).record_bool(&field, true);
                    let record = LogRecord::decode(buf.as_ref()).expect("valid cutoff record");
                    let expected = fits.then(|| {
                        if array {
                            AnyValue::new_array(if complete {
                                vec![]
                            } else {
                                vec![AnyValue::new_string("[\u{2020}]")]
                            })
                        } else {
                            AnyValue::new_kvlist(if complete {
                                vec![]
                            } else {
                                vec![KeyValue::new(
                                    "[\u{2020}]",
                                    AnyValue::new_string("[\u{2020}]"),
                                )]
                            })
                        }
                    });
                    assert_eq!(record.body, expected);
                    assert_eq!(
                        record.attributes,
                        vec![KeyValue::new("b", AnyValue::new_bool(true))]
                    );
                }
            }
        }
    }

    /// Scenario: four members fit without reserved footer space, but a fifth cannot fit.
    /// Guarantees: complete containers keep all four; failed containers reclaim only the suffix after the latest marker-safe boundary and emit one dagger.
    #[test]
    fn structured_cutoff_reclaims_completed_suffix() {
        let values = [("a", 1), ("b", 2), ("c", 3), ("d", 4), ("e", 5)];
        for (array, limit, retained) in [(true, 24, 2), (false, 56, 3)] {
            for complete in [false, true] {
                let mut buf = ProtoBuffer::with_capacity_and_limit(limit, limit);
                let mut completed = 0;
                let count = if complete { 4 } else { 5 };
                let result = buf.try_atomic(|buf| {
                    bounded::value_encoder(buf, LOG_RECORD_BODY, 0, &mut |encoder| {
                        encoder.encode_structured(|w| {
                            if array {
                                w.encode_array(&mut |a| {
                                    for (_, value) in &values[..count] {
                                        a.encode_element(&mut |v| v.encode_i64(*value))?;
                                        completed += 1;
                                    }
                                    Ok(())
                                })
                            } else {
                                w.encode_map(&mut |m| {
                                    for (key, value) in &values[..count] {
                                        m.encode_entry(key, &mut |v| v.encode_i64(*value))?;
                                        completed += 1;
                                    }
                                    Ok(())
                                })
                            }
                        })
                    })
                });
                assert_eq!(completed, 4);
                assert_eq!(
                    result,
                    Ok(if complete {
                        Truncation::None
                    } else {
                        Truncation::Some
                    })
                );
                assert!(buf.len() <= limit);
                let kept = if complete { 4 } else { retained };
                let expected = if array {
                    let mut elements = values[..kept]
                        .iter()
                        .map(|(_, value)| AnyValue::new_int(*value))
                        .collect::<Vec<_>>();
                    if !complete {
                        elements.push(AnyValue::new_string("[\u{2020}]"));
                    }
                    AnyValue::new_array(elements)
                } else {
                    let mut entries = values[..kept]
                        .iter()
                        .map(|(key, value)| KeyValue::new(*key, AnyValue::new_int(*value)))
                        .collect::<Vec<_>>();
                    if !complete {
                        entries.push(KeyValue::new(
                            "[\u{2020}]",
                            AnyValue::new_string("[\u{2020}]"),
                        ));
                    }
                    AnyValue::new_kvlist(entries)
                };
                let record = LogRecord::decode(buf.as_ref()).expect("valid reclaimed prefix");
                assert_eq!(record.body, Some(expected));
            }
        }
    }

    /// Scenario: a child map can encode a scalar entry but cannot fit its own cutoff marker.
    /// Guarantees: child failure rolls back the whole member, the nearest enclosing map emits one dagger, and the failed child's bytes never leak.
    #[test]
    fn structured_cutoff_uses_nearest_marker_safe_container() {
        let mut buf = StackProtoBuffer::<64>::default();
        let mut completed = false;
        let result = buf.try_atomic(|buf| {
            bounded::value_encoder(buf, LOG_RECORD_BODY, 0, &mut |encoder| {
                encoder.encode_structured(|w| {
                    w.encode_map(&mut |m| {
                        m.encode_entry("child", &mut |v| {
                            v.encode_map(&mut |m| {
                                m.encode_entry("x", &mut |v| v.encode_i64(1))?;
                                completed = true;
                                Err(EncodeFailure::Dropped)
                            })
                        })?;
                        panic!("unmarked child failure must stop its parent")
                    })
                })
            })
        });
        assert!(completed);
        assert_eq!(result, Ok(Truncation::Some));
        let record = LogRecord::decode(buf.as_ref()).expect("valid enclosing cutoff");
        assert_eq!(
            record.body,
            Some(AnyValue::new_kvlist(vec![KeyValue::new(
                "[\u{2020}]",
                AnyValue::new_string("[\u{2020}]"),
            )]))
        );
    }

    /// Scenario: a structured body or attribute contains both an incomplete array and an incomplete map.
    /// Guarantees: only the incomplete children get markers across all framing widths; parent siblings survive and multiple child losses count as one attribute loss.
    #[test]
    fn structured_cutoffs_mark_only_incomplete_children() {
        struct PartialChildren;
        impl StructuredValue for PartialChildren {
            fn encode(&self, w: &mut dyn ValueWriter) -> EncodeResult {
                w.encode_map(&mut |m| {
                    m.encode_entry("array", &mut |v| {
                        v.encode_array(&mut |a| {
                            a.encode_element(&mut |v| v.encode_bool(true))?;
                            Err(EncodeFailure::Dropped)
                        })
                    })?;
                    m.encode_entry("map", &mut |v| {
                        v.encode_map(&mut |m| {
                            m.encode_entry("first", &mut |v| v.encode_bool(true))?;
                            Err(EncodeFailure::Dropped)
                        })
                    })?;
                    m.encode_entry("tail", &mut |v| v.encode_bool(true))
                })
            }
        }
        let value = StructBox::new(PartialChildren);
        let attribute = BUDGET_TEST_METADATA
            .fields()
            .field("a")
            .expect("known field");
        let message = MESSAGE_TEST_METADATA
            .fields()
            .field("message")
            .expect("known field");
        let expected = AnyValue::new_kvlist(vec![
            KeyValue::new(
                "array",
                AnyValue::new_array(vec![
                    AnyValue::new_bool(true),
                    AnyValue::new_string("[\u{2020}]"),
                ]),
            ),
            KeyValue::new(
                "map",
                AnyValue::new_kvlist(vec![
                    KeyValue::new("first", AnyValue::new_bool(true)),
                    KeyValue::new("[\u{2020}]", AnyValue::new_string("[\u{2020}]")),
                ]),
            ),
            KeyValue::new("tail", AnyValue::new_bool(true)),
        ]);
        for field in [&attribute, &message] {
            for limit in [1024, 1 << 16, 1 << 23, 1 << 30] {
                let mut buf = ProtoBuffer::with_capacity_and_limit(256, limit);
                let mut visitor = DirectFieldVisitor::new(&mut buf);
                visitor.record_error(field, value.as_struct());
                assert_eq!(
                    visitor.dropped_count(),
                    u32::from(field.name() != "message")
                );
                assert!(buf.len() <= limit);
                let record = LogRecord::decode(buf.as_ref()).expect("valid marked children");
                if field.name() == "message" {
                    assert_eq!(record.body, Some(expected.clone()));
                    assert!(record.attributes.is_empty());
                } else {
                    assert_eq!(
                        record.attributes,
                        vec![KeyValue::new("a", expected.clone())]
                    );
                    assert!(record.body.is_none());
                }
            }
        }
    }

    /// Scenario: a top-level structured log key is too large for its budget.
    /// Guarantees: the failed attribute rolls back, its callback is skipped, and a later small attribute fits without increasing the loss count.
    #[test]
    fn top_level_attribute_budgets_are_independent() {
        let fields = tracing::field::FieldSet::new(
            &[
                "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                "small",
            ],
            tracing::callsite::Identifier(&BUDGET_TEST_CALLSITE),
        );
        let oversized = fields.iter().next().expect("oversized field");
        let small = fields.field("small").expect("known field");
        let mut buf = ProtoBuffer::with_capacity_and_limit(64, 64);
        let mut visitor = DirectFieldVisitor::new(&mut buf);
        visitor.record_toplevel_field(&oversized, |_| {
            panic!("oversized key skips its value");
        });
        assert_eq!(visitor.dropped_count(), 1);
        assert!(visitor.buf.is_empty());
        visitor.record_toplevel_field(&small, |buf| {
            ValueEncoder::new(buf, 0).encode_structured(|w| w.encode_bool(true))
        });
        assert_eq!(visitor.dropped_count(), 1);
        let attributes = LogRecord::decode(buf.as_ref()).expect("log").attributes;
        assert_eq!(
            attributes,
            vec![KeyValue::new("small", AnyValue::new_bool(true))]
        );
    }

    fn encode_chain(
        w: &mut dyn ValueWriter,
        depth: usize,
        visited: &mut usize,
        array: bool,
    ) -> EncodeResult {
        if depth == 0 {
            w.encode_bool(true)
        } else if array {
            w.encode_array(&mut |a| {
                *visited += 1;
                a.encode_element(&mut |v| encode_chain(v, depth - 1, visited, array))?;
                a.encode_element(&mut |v| v.encode_bool(true))
            })
        } else {
            w.encode_map(&mut |m| {
                *visited += 1;
                m.encode_entry("child", &mut |v| encode_chain(v, depth - 1, visited, array))?;
                m.encode_entry("tail", &mut |v| v.encode_bool(true))
            })
        }
    }

    /// Scenario: structured bodies and attributes reach the depth boundary with ample byte budgets.
    /// Guarantees: scalars at MAX_DEPTH fit; deeper maps and arrays become cutoff strings without running their callbacks, preserving tails and counting only attribute loss.
    #[test]
    fn structured_depth_limit_is_independent_of_byte_budget() {
        let attribute = BUDGET_TEST_METADATA
            .fields()
            .field("a")
            .expect("known field");
        let message = MESSAGE_TEST_METADATA
            .fields()
            .field("message")
            .expect("known field");
        for field in [&attribute, &message] {
            for array in [false, true] {
                for depth in [0, MAX_DEPTH - 1, MAX_DEPTH, MAX_DEPTH + 1] {
                    let mut buf = ProtoBuffer::with_capacity(256);
                    let mut visited = 0;
                    let mut visitor = DirectFieldVisitor::new(&mut buf);
                    visitor.record_toplevel_field(field, |buf| {
                        ValueEncoder::new(buf, 0)
                            .encode_structured(|w| encode_chain(w, depth, &mut visited, array))
                    });
                    let truncated = depth > MAX_DEPTH;
                    assert_eq!(
                        visitor.dropped_count(),
                        u32::from(truncated && field.name() != "message")
                    );
                    assert_eq!(visited, depth.min(MAX_DEPTH));
                    let mut expected = if truncated {
                        AnyValue::new_string("[\u{22f1}]")
                    } else {
                        AnyValue::new_bool(true)
                    };
                    for _ in 0..depth.min(MAX_DEPTH) {
                        expected = if array {
                            AnyValue::new_array(vec![expected, AnyValue::new_bool(true)])
                        } else {
                            AnyValue::new_kvlist(vec![
                                KeyValue::new("child", expected),
                                KeyValue::new("tail", AnyValue::new_bool(true)),
                            ])
                        };
                    }
                    let record =
                        LogRecord::decode(buf.as_ref()).expect("valid depth-limited value");
                    if field.name() == "message" {
                        assert_eq!(record.body, Some(expected));
                        assert!(record.attributes.is_empty());
                    } else {
                        assert_eq!(record.attributes, vec![KeyValue::new("a", expected)]);
                        assert!(record.body.is_none());
                    }
                }
            }
        }
    }

    /// Scenario: a depth-cutoff body has exactly enough space for its marker or one byte too little.
    /// Guarantees: maps and arrays emit the complete marker and report truncation, or roll back without invoking their callbacks; later fields remain encodable.
    #[test]
    fn structured_depth_marker_is_atomic() {
        for array in [false, true] {
            for limit in [8, 9] {
                let mut buf = StackProtoBuffer::<32>::default();
                let result = buf.with_max_remaining(limit, |buf| {
                    buf.try_atomic(|buf| {
                        bounded::value_encoder(buf, LOG_RECORD_BODY, MAX_DEPTH, &mut |encoder| {
                            encoder.encode_structured(|w| {
                                if array {
                                    w.encode_array(&mut |_| panic!("depth-exhausted array"))
                                } else {
                                    w.encode_map(&mut |_| panic!("depth-exhausted map"))
                                }
                            })
                        })
                    })
                });
                if limit == 9 {
                    assert_eq!(result, Ok(Truncation::Some));
                } else {
                    assert_eq!(result, Err(EncodeFailure::Dropped));
                    assert!(buf.is_empty());
                }
                assert!(buf.len() <= limit);
                let field = BUDGET_TEST_METADATA
                    .fields()
                    .field("b")
                    .expect("known field");
                DirectFieldVisitor::new(&mut buf).record_bool(&field, true);
                let record = LogRecord::decode(buf.as_ref()).expect("valid cutoff record");
                assert_eq!(
                    record.body,
                    (limit == 9).then(|| AnyValue::new_string("[\u{22f1}]"))
                );
                assert_eq!(
                    record.attributes,
                    vec![KeyValue::new("b", AnyValue::new_bool(true))]
                );
            }
        }
    }

    static BUDGET_TEST_CALLSITE: BudgetTestCallsite = BudgetTestCallsite;

    struct BudgetTestCallsite;

    impl tracing::Callsite for BudgetTestCallsite {
        fn set_interest(&self, _: tracing::subscriber::Interest) {}
        fn metadata(&self) -> &tracing::Metadata<'_> {
            &BUDGET_TEST_METADATA
        }
    }

    static BUDGET_TEST_METADATA: tracing::Metadata<'static> = tracing::Metadata::new(
        "budget_test",
        "slogr",
        Level::INFO,
        Some(file!()),
        Some(line!()),
        Some(module_path!()),
        tracing::field::FieldSet::new(
            &["a", "b", "c", "d"],
            tracing::callsite::Identifier(&BUDGET_TEST_CALLSITE),
        ),
        tracing::metadata::Kind::EVENT,
    );

    /// Scenario: a length-delimited callback writes five bytes and reports successful truncation.
    /// Guarantees: the length is patched to five and a subsequent field can still be appended.
    #[test]
    fn encode_len_delimited_partial_patches_length_on_err() {
        let mut buf = StackProtoBuffer::<128>::default();
        // Pre-write a marker field so we can detect mis-parsing of trailing data.
        buf.encode_string(7, "marker").unwrap();
        let len_before = buf.len();

        let r: Truncation = buf
            .encode_len_delimited(1, |b| {
                b.extend_from_slice(b"abcde")?;
                Ok::<_, EncodeFailure>(Truncation::Some)
            })
            .unwrap();
        assert_eq!(r, Truncation::Some);

        // Read back: tag for field 1 (LEN), then patched length, then 5 bytes.
        let bytes = &buf.as_ref()[len_before..];
        assert_eq!(bytes[0], (1 << 3) | wire_types::LEN as u8);
        // Placeholder width 1 expected (small remaining after marker).
        let payload_len = bytes[1] as usize;
        assert_eq!(payload_len, 5, "length patched to written-bytes count");
        assert_eq!(&bytes[2..2 + payload_len], b"abcde");

        // Append another field after the partial one and confirm it parses.
        buf.encode_string(8, "after").unwrap();
        // Find "after" by scanning -- it must be intact at the end.
        assert!(
            buf.as_ref().windows(5).any(|w| w == b"after"),
            "subsequent field is intact when partial used"
        );
    }

    /// Scenario: strings fully fit, require truncation, or cannot fit in a bounded buffer.
    /// Guarantees: full writes have no suffix, truncation adds a dagger suffix, and hard failure changes no bytes.
    #[test]
    fn encode_string_truncating_three_outcomes() {
        // (a) Full fit -> Ok(false), no suffix.
        let mut buf = StackProtoBuffer::<64>::default();
        let r = bounded::string_truncating(&mut buf, 1, "hi");
        assert!(matches!(r, Ok(Truncation::None)));
        assert!(!buf.as_ref().windows(5).any(|w| w == TRUNCATION_SUFFIX));

        // (b) Truncated -> Ok(true), output ends with suffix.
        let mut buf = StackProtoBuffer::<32>::default();
        let long = "X".repeat(1000);
        let r = bounded::string_truncating(&mut buf, 1, &long);
        assert!(matches!(r, Ok(Truncation::Some)));
        assert!(buf.as_ref().ends_with(TRUNCATION_SUFFIX));

        // (c) Hard fail -> Dropped, buffer unchanged.
        let mut buf = StackProtoBuffer::<4>::default();
        // Pre-fill so remaining < min_overhead.
        buf.extend_from_slice(b"abcd").unwrap();
        let snapshot: Vec<u8> = buf.as_ref().to_vec();
        let r = bounded::string_truncating(&mut buf, 1, &long);
        assert!(matches!(r, Err(EncodeFailure::Dropped)));
        assert_eq!(
            buf.as_ref(),
            snapshot.as_slice(),
            "no partial bytes on hard fail"
        );
    }

    /// Scenario: a long string is truncated while encoding an AnyValue into a bounded buffer.
    /// Guarantees: prost decodes a string containing the original prefix and the dagger suffix.
    #[test]
    fn encode_string_truncating_round_trips_via_prost() {
        let mut buf = StackProtoBuffer::<48>::default();
        let long = "Y".repeat(500);
        let r = bounded::string_truncating(&mut buf, 1, &long);
        assert!(matches!(r, Ok(Truncation::Some)));

        // Decode as AnyValue (field 1 = string_value).
        let av = AnyValue::decode(buf.to_bytes()).expect("valid wire bytes");
        let s = match av.value {
            Some(StringValue(s)) => s,
            other => panic!("unexpected AnyValue variant: {:?}", other),
        };
        assert!(
            s.ends_with("[\u{2020}]"),
            "decoded value should end with truncation suffix"
        );
        assert!(s.starts_with("YY"));
    }

    /// Scenario: a string of four-byte UTF-8 characters is truncated at every possible byte budget.
    /// Guarantees: each result is a character-aligned prefix within budget, or the full input if it fits.
    #[test]
    fn truncate_utf8_returns_char_boundary_prefix() {
        // 4-byte UTF-8 characters (U+1F600, 4 bytes each).
        let s: String = std::iter::repeat_n('\u{1F600}', 5).collect();
        assert_eq!(s.len(), 20);

        // For every possible max_bytes, the result must be:
        //   - a valid &str prefix of `s` (implicit: no panic on slicing)
        //   - of length <= max_bytes
        //   - of length >= max_bytes - 3 (since chars are at most 4 bytes)
        for max_bytes in 0..=s.len() {
            let out = bounded::truncate_utf8(&s, max_bytes);
            assert!(out.len() <= max_bytes, "max_bytes={max_bytes}");
            assert!(
                max_bytes < 4 || out.len() >= max_bytes - 3,
                "max_bytes={max_bytes}, out.len()={}",
                out.len()
            );
            assert!(s.starts_with(out));
            // out.len() must be a multiple of 4 (each char is 4 bytes).
            assert_eq!(out.len() % 4, 0, "max_bytes={max_bytes}");
        }

        // Above the input length, returns the whole string.
        assert_eq!(bounded::truncate_utf8(&s, 1000), s.as_str());
    }
}

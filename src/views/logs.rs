// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0
// Adapted for slogr; see NOTICE for provenance and extraction changes.

//! Borrowed view of an encoded log body and attributes.

use super::decode::{
    FieldRanges, ProtoBytesParser, RepeatedFieldProtoBytesParser,
    from_option_nonzero_range_to_primitive, read_dropped_count, to_nonzero_range,
};
use super::{
    LogRecordView,
    common::{KeyValueIter, RawAnyValue, RawKeyValue},
};
use crate::consts::{field_num::logs::*, wire_types};
use std::{cell::Cell, num::NonZeroUsize};

/// Implementation of `LogRecordView` backed by protobuf serialized `LogRecord` message
pub struct RawLogRecord<'a> {
    bytes_parser: ProtoBytesParser<'a, LogFieldOffsets>,
    dropped_attributes_count: Option<u32>,
}

impl<'a> RawLogRecord<'a> {
    /// Create a new instance of `RawLogRecord`. This is exposed
    /// specifically for interpreting internally generated log records
    /// which encode body and attributes as OTLP bytes.
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            bytes_parser: ProtoBytesParser::new(buf),
            dropped_attributes_count: None,
        }
    }

    pub(crate) fn with_dropped_attributes_count(mut self, count: u32) -> Self {
        self.dropped_attributes_count = Some(count);
        self
    }
}

/// Known field offsets within byte buffer for fields in ResourceLogs message
pub struct LogFieldOffsets {
    scalar_fields: [Cell<Option<(NonZeroUsize, NonZeroUsize)>>; 13],
    first_attribute: Cell<Option<(NonZeroUsize, NonZeroUsize)>>,
}

impl FieldRanges for LogFieldOffsets {
    fn new() -> Self {
        Self {
            scalar_fields: std::array::from_fn(|_| Cell::new(None)),
            first_attribute: Cell::new(None),
        }
    }

    fn set_field_range(&self, field_num: u64, wire_type: u64, start: usize, end: usize) {
        const WIRE_TYPES: [u64; 13] = [
            0,                   // unused
            wire_types::FIXED64, // time_unix_nano = 1
            wire_types::VARINT,  // severity_number = 2
            wire_types::LEN,     // severity_text = 3
            0,                   // unused
            wire_types::LEN,     // body = 5
            wire_types::LEN,     // attributes = 6
            wire_types::VARINT,  // dropped_attributes_count = 7
            wire_types::FIXED32, // flags = 8
            wire_types::LEN,     // trace_id = 9
            wire_types::LEN,     // span_id = 10
            wire_types::FIXED64, // observed_time_unix_nano = 11
            wire_types::LEN,     // event_name = 12
        ];

        let range = match to_nonzero_range(start, end) {
            Some(range) => Some(range),
            None => return,
        };

        if field_num == LOG_RECORD_ATTRIBUTES {
            if self.first_attribute.get().is_none() && wire_type == wire_types::LEN {
                self.first_attribute.set(range);
            }
        } else if field_num < 13 {
            let idx = field_num as usize;
            if wire_type == WIRE_TYPES[idx] {
                self.scalar_fields[idx].set(range);
            }
        }
    }

    #[inline]
    fn get_field_range(&self, field_num: u64) -> Option<(usize, usize)> {
        let range = if field_num == LOG_RECORD_ATTRIBUTES {
            self.first_attribute.get()
        } else {
            self.scalar_fields
                .get(field_num as usize)
                .and_then(|c| c.get())
        };

        from_option_nonzero_range_to_primitive(range)
    }
}

impl LogRecordView for RawLogRecord<'_> {
    type Attribute<'att>
        = RawKeyValue<'att>
    where
        Self: 'att;

    type AttributeIter<'att>
        = KeyValueIter<'att, LogFieldOffsets>
    where
        Self: 'att;

    type Body<'bod>
        = RawAnyValue<'bod>
    where
        Self: 'bod;

    #[inline]
    fn attributes(&self) -> Self::AttributeIter<'_> {
        KeyValueIter::new(RepeatedFieldProtoBytesParser::from_byte_parser(
            &self.bytes_parser,
            LOG_RECORD_ATTRIBUTES,
            wire_types::LEN,
        ))
    }

    #[inline]
    fn body(&self) -> Option<Self::Body<'_>> {
        self.bytes_parser
            .advance_to_find_field(LOG_RECORD_BODY)
            .map(RawAnyValue::new)
    }

    #[inline]
    fn dropped_attributes_count(&self) -> u32 {
        if let Some(count) = self.dropped_attributes_count {
            return count;
        }
        let slice = self
            .bytes_parser
            .advance_to_find_field(LOG_RECORD_DROPPED_ATTRIBUTES_COUNT);
        read_dropped_count(slice)
    }
}

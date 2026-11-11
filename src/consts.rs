// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0
// Adapted for slogr; see NOTICE for provenance and extraction changes.

//! Constants related to OTLP proto messages such as field numbers and other proto constants like
//! wire types

/// Protobuf wire types
pub mod wire_types {
    /// Varint (int32, int64, uint32, uint64, sint32, sint64, bool)
    pub const VARINT: u64 = 0;

    /// 64-bit (fixed64, sfixed64, double)
    pub const FIXED64: u64 = 1;

    /// Length-delimited (string, bytes, embedded messages)
    pub const LEN: u64 = 2;

    // Tags 3 and 4 have an obsolete definition, will never be seen.

    /// 32-bit (fixed32, sfixed32, float)
    pub const FIXED32: u64 = 5;
}

/// field number for OTLP protobuf messages
pub mod field_num {
    #[allow(missing_docs)]
    pub mod common {
        pub const KEY_VALUE_KEY: u64 = 1;
        pub const KEY_VALUE_VALUE: u64 = 2;

        pub const ANY_VALUE_STRING_VALUE: u64 = 1;
        pub const ANY_VALUE_BOOL_VALUE: u64 = 2;
        pub const ANY_VALUE_INT_VALUE: u64 = 3;
        pub const ANY_VALUE_DOUBLE_VALUE: u64 = 4;
        pub const ANY_VALUE_ARRAY_VALUE: u64 = 5;
        pub const ANY_VALUE_KVLIST_VALUE: u64 = 6;
        pub const ANY_VALUE_BYTES_VALUE: u64 = 7;

        pub const KEY_VALUE_LIST_VALUES: u64 = 1;
        pub const ARRAY_VALUE_VALUES: u64 = 1;
    }

    #[allow(missing_docs)]
    pub mod logs {

        pub const LOG_RECORD_TIME_UNIX_NANO: u64 = 1;
        pub const LOG_RECORD_SEVERITY_NUMBER: u64 = 2;
        pub const LOG_RECORD_BODY: u64 = 5;
        pub const LOG_RECORD_ATTRIBUTES: u64 = 6;
        pub const LOG_RECORD_DROPPED_ATTRIBUTES_COUNT: u64 = 7;
        pub const LOG_RECORD_EVENT_NAME: u64 = 12;
    }
}

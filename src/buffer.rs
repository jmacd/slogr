// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0
// Adapted for slogr; see NOTICE for provenance and extraction changes.

//! Bounded protobuf buffers with heap and stack storage.

use crate::consts::wire_types;
use bytes::Bytes;
use std::fmt;

/// Returned when a buffer write cannot fit within the size limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EncodeFailure {
    /// Dropped due to space limit.
    Dropped,
}

impl std::error::Error for EncodeFailure {}

impl fmt::Display for EncodeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dropped => write!(f, "dropped"),
        }
    }
}

/// Result type for encoding methods that can fail only because of a size limit.
pub type EncodeResult<T = ()> = Result<T, EncodeFailure>;

const fn tag_width_limit(tagw: usize) -> usize {
    1 << (7 * tagw)
}

const MAX_TAG_WIDTH: usize = 4;

/// The maximum supported protocol buffer size limit (1 << 28) = 256MiB.
pub const MAX_OTLP_SIZE_LIMIT: usize = tag_width_limit(MAX_TAG_WIDTH);

/// Build an `N`-byte zero-valued varint placeholder at compile time.
const fn make_len_placeholder<const N: usize>() -> [u8; N] {
    let mut arr = [0x80u8; N];
    arr[N - 1] = 0x00;
    arr
}

/// Trait for buffers that support bounded protobuf encoding.
///
/// The trait exposes a small set of storage primitives; all higher-level
/// encoding methods are provided as default methods so that two concrete
/// storage strategies (heap-backed [`ProtoBuffer`] and stack-backed
/// [`StackProtoBuffer`]) can share the encoding logic.
///
/// Write methods return [`EncodeResult`], enabling `?` propagation through
/// recursive protobuf encoding. Callers are responsible for counting
/// dropped fields. The [`try_atomic`](Self::try_atomic) method provides
/// atomic field-level writes: either the entire field is written, or the
/// buffer is rolled back to its previous state.
///
pub trait BoundedBuf {
    /// Current write position (encoded byte count).
    fn len(&self) -> usize;

    /// Maximum byte count the buffer will accept.
    fn limit(&self) -> usize;

    /// Set a new write limit.
    fn set_limit(&mut self, limit: usize);

    /// Append bytes if they fit within the limit; otherwise return `Err(Dropped)`
    /// without writing any bytes.
    fn try_extend_slice(&mut self, slice: &[u8]) -> EncodeResult;

    /// Truncate the buffer to the given position. Used for rollback.
    fn truncate(&mut self, pos: usize);

    /// Borrow the encoded bytes.
    fn as_slice(&self) -> &[u8];

    /// Borrow the encoded bytes mutably to patch length placeholders.
    fn as_mut_slice(&mut self) -> &mut [u8];

    /// Reset the buffer to empty (preserves any underlying allocation).
    fn clear(&mut self);

    /// Returns true if no bytes have been written.
    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of bytes remaining before the limit.
    #[inline]
    fn remaining(&self) -> usize {
        self.limit().saturating_sub(self.len())
    }

    /// Fallible alias for [`Self::try_extend_slice`], unlike `Vec::extend_from_slice`.
    #[inline]
    fn extend_from_slice(&mut self, slice: &[u8]) -> EncodeResult {
        self.try_extend_slice(slice)
    }

    /// Encodes a varint containing type (3 bits) and tag value.
    #[inline]
    fn encode_field_tag(&mut self, tag: u64, wire_type: u64) -> EncodeResult {
        let key = (tag << 3) | wire_type;
        self.encode_varint(key)
    }

    /// An unsigned varint encoding.
    #[inline(always)]
    fn encode_varint(&mut self, value: u64) -> EncodeResult {
        if value < tag_width_limit(1) as u64 {
            return self.try_extend_slice(&[value as u8]);
        }
        if value < tag_width_limit(2) as u64 {
            return self.try_extend_slice(&[((value & 0x7F) | 0x80) as u8, (value >> 7) as u8]);
        }
        let mut tmp = [0u8; 10];
        let mut i = 0;
        let mut v = value;
        while v >= 0x80 {
            tmp[i] = ((v & 0x7F) | 0x80) as u8;
            i += 1;
            v >>= 7;
        }
        tmp[i] = v as u8;
        i += 1;
        self.try_extend_slice(&tmp[..i])
    }

    /// Encodes the signed varint type (e.g. sint32, sint64) using zig-zag encoding.
    /// <https://protobuf.dev/programming-guides/encoding/#signed-ints>
    #[inline]
    fn encode_sint32(&mut self, value: i32) -> EncodeResult {
        self.encode_varint(((value << 1) ^ (value >> 31)) as u64)
    }

    /// Encode a string field by tag number.
    #[inline]
    fn encode_string(&mut self, tag: u64, val: &str) -> EncodeResult {
        self.encode_field_tag(tag, wire_types::LEN)?;
        self.encode_varint(val.len() as u64)?;
        self.try_extend_slice(val.as_bytes())
    }

    /// Encode a bytes field by tag number.
    #[inline]
    fn encode_bytes(&mut self, tag: u64, val: &[u8]) -> EncodeResult {
        self.encode_field_tag(tag, wire_types::LEN)?;
        self.encode_varint(val.len() as u64)?;
        self.try_extend_slice(val)
    }

    /// Returns the narrowest placeholder width sufficient for this
    /// buffer's remaining space.
    #[inline]
    fn placeholder_width(&self) -> usize {
        let rem = self.remaining();
        if rem < tag_width_limit(1) {
            1
        } else if rem < tag_width_limit(2) {
            2
        } else if rem < tag_width_limit(3) {
            3
        } else {
            4
        }
    }

    /// Encode a length-delimited field with variable-width placeholder. Does not
    /// roll-backup; supports truncation or not.
    #[inline]
    fn encode_len_delimited<T, E: From<EncodeFailure>>(
        &mut self,
        tag: u64,
        f: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E>
    where
        Self: Sized,
    {
        match self.placeholder_width() {
            1 => self.encode_len_delimited_width::<1, _, E, _>(tag, f),
            2 => self.encode_len_delimited_width::<2, _, E, _>(tag, f),
            3 => self.encode_len_delimited_width::<3, _, E, _>(tag, f),
            _ => self.encode_len_delimited_width::<4, _, E, _>(tag, f),
        }
    }

    /// Implementation of encode_len_delimited with an explicit
    /// placeholder width. Supports TruncateResult or EncodeResult.
    #[inline]
    fn encode_len_delimited_width<const B: usize, T, E, F>(
        &mut self,
        tag: u64,
        f: F,
    ) -> Result<T, E>
    where
        E: From<EncodeFailure>,
        F: FnOnce(&mut Self) -> Result<T, E>,
        Self: Sized,
    {
        self.encode_field_tag(tag, wire_types::LEN)?;
        let len_start_pos = self.len();
        self.try_extend_slice(&make_len_placeholder::<B>())?;
        let ok = f(self)?;
        let len = self.len() - len_start_pos - B;
        patch_len_placeholder::<B, _>(self, len, len_start_pos);
        Ok(ok)
    }

    /// Run a closure with a temporarily reduced limit.
    ///
    /// The buffer's `limit` is set to `min(current_limit, len() + max_remaining)`
    /// for the duration of the closure, and restored afterward. This lets
    /// callers cap how many bytes a single nested encoding may consume,
    /// without touching the underlying buffer's outer limit.
    #[inline]
    fn with_max_remaining<R>(&mut self, max_remaining: usize, f: impl FnOnce(&mut Self) -> R) -> R
    where
        Self: Sized,
    {
        let saved = self.limit();
        let scoped = self.len().saturating_add(max_remaining).min(saved);
        self.set_limit(scoped);
        let result = f(self);
        self.set_limit(saved);
        result
    }

    /// Try to encode a complete field atomically.
    ///
    /// If the closure returns an error, the buffer is rolled back to
    /// its state before the call and the error is returned.
    #[inline]
    fn try_atomic<T, E>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, E>) -> Result<T, E>
    where
        Self: Sized,
    {
        let cp = self.len();
        match f(self) {
            Ok(t) => Ok(t),
            Err(e) => {
                self.truncate(cp);
                Err(e)
            }
        }
    }
}

/// Heap-backed protobuf encoding buffer.
///
/// Backed by a `Vec<u8>`. Use [`with_capacity`](Self::with_capacity) or
/// [`with_capacity_and_limit`](Self::with_capacity_and_limit) to pre-allocate
/// when the maximum size is known to avoid Vec growth on the hot path.
#[derive(Debug)]
pub struct ProtoBuffer {
    buffer: Vec<u8>,
    limit: usize,
}

impl Default for ProtoBuffer {
    fn default() -> Self {
        Self {
            buffer: Vec::new(),
            limit: MAX_OTLP_SIZE_LIMIT,
        }
    }
}

impl ProtoBuffer {
    /// Construct an unbounded buffer with starting allocation of `capacity` bytes.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(MAX_OTLP_SIZE_LIMIT.min(capacity)),
            limit: MAX_OTLP_SIZE_LIMIT,
        }
    }

    /// Pre-allocate `capacity` bytes and bound writes to `limit`.
    ///
    /// Used by callers that know the bound up front (e.g. self-tracing log
    /// records) to avoid both Vec growth and per-write storage-discriminator
    /// branches that would arise from a SmallVec-like inline option.
    #[must_use]
    pub fn with_capacity_and_limit(capacity: usize, limit: usize) -> Self {
        let limit = MAX_OTLP_SIZE_LIMIT.min(limit);
        let cap = MAX_OTLP_SIZE_LIMIT.min(capacity).min(limit);
        Self {
            buffer: Vec::with_capacity(cap),
            limit,
        }
    }

    /// Returns the current capacity of the underlying allocation.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.buffer.capacity()
    }

    /// Returns the current length in bytes (inherent shortcut for [`BoundedBuf::len`]).
    #[must_use]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Returns true if the buffer has no bytes (inherent shortcut for [`BoundedBuf::is_empty`]).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Reset the buffer length to zero, keeping allocated capacity.
    /// Inherent shortcut for [`BoundedBuf::clear`].
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Consume the buffer and return its bytes as a `Bytes` (zero-copy).
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        Bytes::from(self.buffer)
    }

    /// Consume the buffer and return its inner `Vec<u8>`.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.buffer
    }

    /// Take encoded bytes, returning them as `Bytes`, plus the previous capacity
    /// so the caller can re-allocate a same-size buffer for reuse.
    pub fn take_into_bytes(&mut self) -> (Bytes, usize) {
        let buffer = std::mem::take(&mut self.buffer);
        let capacity = buffer.capacity();
        (Bytes::from(buffer), capacity)
    }

    /// Ensure the underlying buffer has at least the requested capacity.
    pub fn ensure_capacity(&mut self, capacity: usize) {
        if capacity > self.buffer.capacity() {
            self.buffer.reserve(capacity - self.buffer.capacity());
        }
    }

    /// Clears the buffer and bounds the allocation retained for later encodes.
    pub fn retain_capacity(&mut self, maximum: usize) {
        self.buffer.clear();
        if self.buffer.capacity() > maximum {
            self.buffer = Vec::with_capacity(maximum.min(self.limit));
        }
    }
}

impl BoundedBuf for ProtoBuffer {
    #[inline]
    fn len(&self) -> usize {
        self.buffer.len()
    }
    #[inline]
    fn limit(&self) -> usize {
        self.limit
    }
    #[inline]
    fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }
    #[inline]
    fn try_extend_slice(&mut self, slice: &[u8]) -> EncodeResult {
        if self.buffer.len() + slice.len() <= self.limit {
            self.buffer.extend_from_slice(slice);
            Ok(())
        } else {
            Err(EncodeFailure::Dropped)
        }
    }
    #[inline]
    fn truncate(&mut self, pos: usize) {
        self.buffer.truncate(pos);
    }
    #[inline]
    fn as_slice(&self) -> &[u8] {
        &self.buffer
    }
    #[inline]
    fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.buffer
    }
    #[inline]
    fn clear(&mut self) {
        self.buffer.clear();
    }
}

impl AsRef<[u8]> for ProtoBuffer {
    fn as_ref(&self) -> &[u8] {
        &self.buffer
    }
}

impl AsMut<[u8]> for ProtoBuffer {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.buffer
    }
}

/// Stack-backed protobuf encoding buffer with fixed `N`-byte capacity.
///
/// Performs zero allocations: storage is an inline `[u8; N]` array with a
/// running cursor. Used by zero-allocation paths like
/// `raw_error!`/`StackLogRecord` where a small bounded buffer is sufficient.
pub struct StackProtoBuffer<const N: usize> {
    buf: [u8; N],
    pos: usize,
    limit: usize,
}

impl<const N: usize> StackProtoBuffer<N> {
    /// Borrow the encoded bytes (zero-copy).
    #[must_use]
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: pos is always <= N and tracks initialized bytes.
        &self.buf[..self.pos]
    }

    /// Copy encoded bytes into a `Bytes` (allocates).
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        Bytes::copy_from_slice(self.as_slice())
    }
}

impl<const N: usize> Default for StackProtoBuffer<N> {
    fn default() -> Self {
        Self {
            buf: [0u8; N],
            pos: 0,
            limit: N,
        }
    }
}

impl<const N: usize> BoundedBuf for StackProtoBuffer<N> {
    #[inline]
    fn len(&self) -> usize {
        self.pos
    }
    #[inline]
    fn limit(&self) -> usize {
        self.limit
    }
    #[inline]
    fn set_limit(&mut self, limit: usize) {
        self.limit = limit.min(N);
    }
    #[inline]
    fn try_extend_slice(&mut self, slice: &[u8]) -> EncodeResult {
        let new_pos = self.pos + slice.len();
        if new_pos <= self.limit {
            self.buf[self.pos..new_pos].copy_from_slice(slice);
            self.pos = new_pos;
            Ok(())
        } else {
            Err(EncodeFailure::Dropped)
        }
    }
    #[inline]
    fn truncate(&mut self, pos: usize) {
        if pos < self.pos {
            self.pos = pos;
        }
    }
    #[inline]
    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.pos]
    }
    #[inline]
    fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.buf[..self.pos]
    }
    #[inline]
    fn clear(&mut self) {
        self.pos = 0;
    }
}

impl<const N: usize> AsRef<[u8]> for StackProtoBuffer<N> {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl<const N: usize> AsMut<[u8]> for StackProtoBuffer<N> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.pos]
    }
}

/// Patch a previously written length placeholder with the actual length.
#[inline]
fn patch_len_placeholder<const N: usize, B: BoundedBuf>(
    buf: &mut B,
    len: usize,
    len_start_pos: usize,
) {
    let slice = buf.as_mut_slice();
    for i in 0..N {
        slice[len_start_pos + i] += ((len >> (i * 7)) & 0x7f) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: the same nested string is encoded with 127-byte, 128-byte, and default buffers.
    /// Guarantees: length placeholders use one, two, and four bytes respectively.
    #[test]
    fn test_encode_len_delimited_auto_width() {
        // A small inline buffer (127 bytes) should use a 1-byte placeholder.
        let mut buf = StackProtoBuffer::<127>::default();
        buf.encode_len_delimited(1, |buf: &mut StackProtoBuffer<127>| -> EncodeResult {
            buf.encode_string(1, "hi")
        })
        .unwrap();
        // tag(1) + placeholder(1) + inner_tag(1) + inner_len_varint(1) + "hi"(2) = 6
        assert_eq!(buf.len(), 6);

        // A 128-byte inline buffer should use a 2-byte placeholder (128 >= 2^7).
        let mut buf = StackProtoBuffer::<128>::default();
        buf.encode_len_delimited(1, |buf: &mut StackProtoBuffer<128>| -> EncodeResult {
            buf.encode_string(1, "hi")
        })
        .unwrap();
        // tag(1) + placeholder(2) + inner_tag(1) + inner_len_varint(1) + "hi"(2) = 7
        assert_eq!(buf.len(), 7);

        // A default (256MiB limit) buffer should use a 4-byte placeholder.
        let mut buf = ProtoBuffer::default();
        buf.encode_len_delimited(1, |buf| -> EncodeResult { buf.encode_string(1, "hi") })
            .unwrap();
        // tag(1) + placeholder(4) + inner_tag(1) + inner_len_varint(1) + "hi"(2) = 9
        assert_eq!(buf.len(), 9);
    }

    /// Scenario: atomic encoding fails after writing, then a length-delimited callback returns a count.
    /// Guarantees: failure restores the original bytes; success retains bytes and the callback's count.
    #[test]
    fn try_atomic_rolls_back_buffer_on_error() {
        let mut buf = ProtoBuffer::default();
        buf.encode_string(1, "before").unwrap();
        let snapshot: Vec<u8> = buf.as_ref().to_vec();
        let snapshot_len = buf.len();

        let res: EncodeResult = buf.try_atomic(|b| {
            // Write several bytes successfully, then fail.
            b.encode_string(2, "partial-content")?;
            b.encode_varint(0xDEAD_BEEF)?;
            Err(EncodeFailure::Dropped)
        });
        assert!(matches!(res, Err(EncodeFailure::Dropped)));
        assert_eq!(buf.len(), snapshot_len, "len must be restored");
        assert_eq!(buf.as_ref(), snapshot.as_slice(), "bytes must be identical");

        // try_atomic on Ok preserves all writes.
        let res: EncodeResult<usize> = buf.try_atomic(|b| {
            b.encode_len_delimited(3, |b| {
                b.encode_string(1, "ok")?;
                Ok(b.len() - snapshot_len)
            })
        });
        assert_eq!(res.unwrap(), buf.len() - snapshot_len);
        assert!(buf.len() > snapshot_len);
    }

    /// Scenario: a length-delimited field overflows an eight-byte buffer during its inner encoding.
    /// Guarantees: failure leaves the length unpatched, while an atomic wrapper rolls back all bytes.
    #[test]
    fn encode_len_delimited_does_not_patch_on_inner_err() {
        let mut buf = StackProtoBuffer::<8>::default();
        let r: EncodeResult = buf.encode_len_delimited(1, |b| {
            // Write enough partial content to force overflow before completion.
            b.encode_string(1, "AAAAAAAAAAAAAAAAAAAA")?;
            Ok(())
        });
        assert!(matches!(r, Err(EncodeFailure::Dropped)));
        // tag(1) + placeholder(1) was written, plus partial content bytes.
        // The placeholder is unpatched (still 0x00), so a parser would read
        // the body as 0 bytes and continue at the partial content as garbage.
        assert!(buf.len() >= 2, "tag + placeholder were written");
        assert_eq!(buf.as_ref()[1], 0x00, "placeholder remained unpatched");

        // The safe pattern: wrap in try_atomic so partial bytes roll back.
        let mut buf = StackProtoBuffer::<8>::default();
        let r: EncodeResult = buf.try_atomic(|b| {
            b.encode_len_delimited(1, |b| b.encode_string(1, "AAAAAAAAAAAAAAAAAAAA"))
        });
        assert!(matches!(r, Err(EncodeFailure::Dropped)));
        assert_eq!(buf.len(), 0, "try_atomic rolled back partial bytes");
    }

    /// Scenario: a buffer temporarily restricts its remaining capacity to eight bytes without writing.
    /// Guarantees: the scoped budget applies and the original limit and remaining capacity are restored.
    #[test]
    fn with_max_remaining_restores_outer_limit() {
        let mut buf = StackProtoBuffer::<64>::default();
        let outer_limit = buf.limit();
        let outer_remaining = buf.remaining();

        let inner_seen = buf.with_max_remaining(8, |b| {
            assert!(b.remaining() <= 8, "scoped limit applied");
            b.remaining()
        });
        assert_eq!(inner_seen, 8.min(outer_remaining));

        assert_eq!(buf.limit(), outer_limit, "outer limit restored");
        assert_eq!(buf.remaining(), outer_remaining);
    }

    /// Scenario: a ten-byte remaining-capacity scope is nested inside a fifty-byte scope.
    /// Guarantees: each budget applies locally and unwinding restores each enclosing limit in turn.
    #[test]
    fn with_max_remaining_nests_correctly() {
        let mut buf = StackProtoBuffer::<128>::default();
        let outer = buf.limit();

        buf.with_max_remaining(50, |b| {
            let mid = b.limit();
            assert!(mid <= b.len() + 50);
            b.with_max_remaining(10, |b| {
                assert!(b.remaining() <= 10);
            });
            // After inner returns, mid-level limit must be restored, not outer.
            assert_eq!(b.limit(), mid, "inner restoration goes to enclosing scope");
        });
        assert_eq!(buf.limit(), outer, "fully restored after both scopes");
    }

    /// Scenario: a bounded buffer receives a scoped remaining-capacity request of usize::MAX.
    /// Guarantees: the request does not overflow or raise the limit beyond the outer buffer limit.
    #[test]
    fn with_max_remaining_saturates_and_honors_outer() {
        let mut buf = StackProtoBuffer::<32>::default();
        let outer = buf.limit();

        // usize::MAX should saturate without overflow and never exceed outer.
        buf.with_max_remaining(usize::MAX, |b| {
            assert_eq!(b.limit(), outer, "scoped limit clamped to outer");
        });
        assert_eq!(buf.limit(), outer);
    }
}

//! Bounds-checked reading over a byte slice.
//!
//! Every field decoded from a save file comes through here. Save files are
//! untrusted input — they can be truncated by a crash mid-write, corrupted on
//! disk, or handed over deliberately malformed — so no read may assume there
//! are bytes left, and no length read out of the file may be used to allocate
//! without first checking it against what actually remains.
//!
//! That last point is the one that matters. A four-byte length prefix claiming
//! four billion entries is trivial to write and, fed straight to
//! `Vec::with_capacity`, is an out-of-memory abort. Every `take_*` here is
//! checked against the real remaining length first.

/// Ran out of input, or a field was self-evidently wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    #[error("needed {needed} bytes at offset {at} but only {available} remain")]
    OutOfBounds {
        at: usize,
        needed: usize,
        available: usize,
    },
    #[error("{field} is {value}, above the {limit} allowed")]
    TooLarge {
        field: &'static str,
        value: u64,
        limit: u64,
    },
    #[error("{field} is not valid UTF-8")]
    NotUtf8 { field: &'static str },
}

type Result<T> = std::result::Result<T, CursorError>;

/// A read head over borrowed bytes.
pub struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Cursor { bytes, at: 0 }
    }

    pub fn position(&self) -> usize {
        self.at
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Advance over `count` bytes, or fail without moving.
    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if count > self.remaining() {
            return Err(CursorError::OutOfBounds {
                at: self.at,
                needed: count,
                available: self.remaining(),
            });
        }
        let slice = &self.bytes[self.at..self.at + count];
        self.at += count;
        Ok(slice)
    }

    pub fn take_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    pub fn take_u8(&mut self) -> Result<u8> {
        Ok(self.take_array::<1>()?[0])
    }

    pub fn take_u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take_array()?))
    }

    pub fn take_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take_array()?))
    }

    pub fn take_u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take_array()?))
    }

    pub fn take_f32(&mut self) -> Result<f32> {
        // NaN and infinity pass through here on purpose: this layer only
        // guarantees the bytes exist. What a non-finite value *means* is the
        // caller's decision — the player decoder, for one, treats a NaN pose
        // as "respawn" rather than refusing the whole file.
        Ok(f32::from_le_bytes(self.take_array()?))
    }

    /// Read a count and check it against a hard cap *and* against how many
    /// bytes could possibly still supply that many items.
    ///
    /// `bytes_per_item` is the smallest an item can be. Passing it is what
    /// stops a plausible-looking count from reserving memory the file could
    /// never fill.
    pub fn take_count(
        &mut self,
        field: &'static str,
        limit: usize,
        bytes_per_item: usize,
    ) -> Result<usize> {
        let count = self.take_u32()? as usize;
        if count > limit {
            return Err(CursorError::TooLarge {
                field,
                value: count as u64,
                limit: limit as u64,
            });
        }
        // Saturating: a huge count times a small size must not wrap into a
        // small product that then passes the check.
        let smallest = count.saturating_mul(bytes_per_item);
        if smallest > self.remaining() {
            return Err(CursorError::OutOfBounds {
                at: self.at,
                needed: smallest,
                available: self.remaining(),
            });
        }
        Ok(count)
    }

    /// Read a `u16`-prefixed UTF-8 string.
    pub fn take_string(&mut self, field: &'static str, limit: usize) -> Result<String> {
        let length = self.take_u16()? as usize;
        if length > limit {
            return Err(CursorError::TooLarge {
                field,
                value: length as u64,
                limit: limit as u64,
            });
        }
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| CursorError::NotUtf8 { field })
    }

    /// Check for an expected magic tag.
    pub fn expect_magic(&mut self, expected: &[u8; 4]) -> Result<bool> {
        Ok(&self.take_array::<4>()? == expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_advance_and_report_what_is_left() {
        let bytes = [1u8, 0, 0, 0, 9];
        let mut cursor = Cursor::new(&bytes);

        assert_eq!(cursor.remaining(), 5);
        assert_eq!(cursor.take_u32().unwrap(), 1);
        assert_eq!(cursor.position(), 4);
        assert_eq!(cursor.take_u8().unwrap(), 9);
        assert!(cursor.is_empty());
    }

    #[test]
    fn little_endian_round_trips_every_width() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0xabcdu16.to_le_bytes());
        bytes.extend_from_slice(&0xdeadbeefu32.to_le_bytes());
        bytes.extend_from_slice(&0x0123456789abcdefu64.to_le_bytes());

        let mut cursor = Cursor::new(&bytes);
        assert_eq!(cursor.take_u16().unwrap(), 0xabcd);
        assert_eq!(cursor.take_u32().unwrap(), 0xdeadbeef);
        assert_eq!(cursor.take_u64().unwrap(), 0x0123456789abcdef);
    }

    #[test]
    fn reading_past_the_end_fails_instead_of_panicking() {
        let bytes = [1u8, 2];
        let mut cursor = Cursor::new(&bytes);

        let error = cursor.take_u32().unwrap_err();
        assert!(matches!(error, CursorError::OutOfBounds { needed: 4, available: 2, .. }));
        // A failed read must not consume anything, or recovery is impossible.
        assert_eq!(cursor.position(), 0);
        assert_eq!(cursor.remaining(), 2);
    }

    #[test]
    fn an_empty_input_yields_errors_not_panics() {
        let mut cursor = Cursor::new(&[]);
        assert!(cursor.take_u8().is_err());
        assert!(cursor.take_u64().is_err());
        assert!(cursor.expect_magic(b"VXRG").is_err());
    }

    #[test]
    fn a_count_beyond_its_cap_is_refused() {
        let bytes = 5000u32.to_le_bytes();
        let mut cursor = Cursor::new(&bytes);

        let error = cursor.take_count("palette", 4096, 1).unwrap_err();
        assert!(matches!(error, CursorError::TooLarge { value: 5000, limit: 4096, .. }));
    }

    #[test]
    fn a_count_the_file_could_not_possibly_supply_is_refused() {
        // The attack this stops: a plausible count used to size an allocation
        // for data that is not there. 1000 items of 8 bytes needs 8000 bytes;
        // the file has four.
        let mut bytes = 1000u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 4]);
        let mut cursor = Cursor::new(&bytes);

        let error = cursor.take_count("blocks", 100_000, 8).unwrap_err();
        assert!(matches!(error, CursorError::OutOfBounds { needed: 8000, available: 4, .. }));
    }

    #[test]
    fn an_enormous_count_cannot_wrap_the_size_calculation() {
        // u32::MAX items of 8 bytes overflows usize on 32-bit and is huge on
        // 64-bit; either way it must be rejected, not wrapped into something
        // small enough to pass.
        let bytes = u32::MAX.to_le_bytes();
        let mut cursor = Cursor::new(&bytes);
        assert!(cursor.take_count("blocks", usize::MAX, 8).is_err());
    }

    #[test]
    fn floats_round_trip_including_the_awkward_ones() {
        let values = [0.0f32, -1.5, f32::MAX, f32::NAN, f32::INFINITY];
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let mut cursor = Cursor::new(&bytes);
        for expected in values {
            let got = cursor.take_f32().unwrap();
            // Bit-exact, which also covers NaN (NaN != NaN by value).
            assert_eq!(got.to_bits(), expected.to_bits());
        }
        assert!(cursor.take_f32().is_err(), "read past the end");
    }

    #[test]
    fn strings_round_trip_and_are_length_checked() {
        let mut bytes = (5u16).to_le_bytes().to_vec();
        bytes.extend_from_slice(b"stone");
        let mut cursor = Cursor::new(&bytes);

        assert_eq!(cursor.take_string("name", 64).unwrap(), "stone");
    }

    #[test]
    fn an_overlong_string_is_refused_before_it_is_read() {
        let mut bytes = (300u16).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[b'a'; 300]);
        let mut cursor = Cursor::new(&bytes);

        let error = cursor.take_string("name", 256).unwrap_err();
        assert!(matches!(error, CursorError::TooLarge { limit: 256, .. }));
    }

    #[test]
    fn invalid_utf8_is_an_error_rather_than_a_replacement_character() {
        // Block names are identities that must round-trip exactly; silently
        // substituting U+FFFD would rename a block and lose the mapping.
        let mut bytes = (2u16).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0xff, 0xfe]);
        let mut cursor = Cursor::new(&bytes);

        assert!(matches!(
            cursor.take_string("name", 64).unwrap_err(),
            CursorError::NotUtf8 { .. }
        ));
    }

    #[test]
    fn magic_tags_are_compared_not_assumed() {
        let mut right = Cursor::new(b"VXRG____");
        assert!(right.expect_magic(b"VXRG").unwrap());

        let mut wrong = Cursor::new(b"NOPE____");
        assert!(!wrong.expect_magic(b"VXRG").unwrap());
    }

    #[test]
    fn truncation_at_every_offset_is_survivable() {
        // The crash-mid-write case: whatever prefix survived must produce an
        // error rather than a panic, at every possible cut point.
        let mut full = Vec::new();
        full.extend_from_slice(b"VXRG");
        full.extend_from_slice(&7u32.to_le_bytes());
        full.extend_from_slice(&(5u16).to_le_bytes());
        full.extend_from_slice(b"stone");

        for cut in 0..full.len() {
            let mut cursor = Cursor::new(&full[..cut]);
            // Any sequence of reads, any prefix: never a panic.
            let _ = cursor.expect_magic(b"VXRG");
            let _ = cursor.take_count("n", 1024, 1);
            let _ = cursor.take_string("name", 256);
        }
    }
}

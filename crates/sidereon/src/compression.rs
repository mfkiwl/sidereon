//! Bounded decoding for gzip member series.
//!
//! RFC 1952 defines a gzip file as a series of one or more members.  This
//! decoder parses every member itself instead of relying on a convenience
//! reader so optional header strings are bounded only by the caller's archive
//! limit, every member trailer is checked, and trailing data is rejected.

use core::fmt;

use crc32fast::Hasher;
use flate2::{Decompress, FlushDecompress, Status};

const GZIP_ID1: u8 = 0x1f;
const GZIP_ID2: u8 = 0x8b;
const DEFLATE_METHOD: u8 = 8;
const FLAG_HEADER_CRC: u8 = 0x02;
const FLAG_EXTRA: u8 = 0x04;
const FLAG_NAME: u8 = 0x08;
const FLAG_COMMENT: u8 = 0x10;
const FLAG_RESERVED: u8 = 0xe0;
const FIXED_HEADER_BYTES: usize = 10;
const TRAILER_BYTES: usize = 8;
const OUTPUT_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GzipLimits {
    pub(crate) max_compressed_bytes: usize,
    pub(crate) max_decompressed_bytes: usize,
}

impl GzipLimits {
    pub(crate) const fn new(max_compressed_bytes: usize, max_decompressed_bytes: usize) -> Self {
        Self {
            max_compressed_bytes,
            max_decompressed_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GzipError {
    /// The caller-provided archive exceeds its compressed-byte limit.
    CompressedSizeExceeded {
        /// Configured maximum archive size.
        limit: usize,
        /// Observed archive size.
        actual: usize,
    },
    /// Cumulative decompressed output exceeds its limit.
    DecompressedSizeExceeded {
        /// Configured maximum decompressed size.
        limit: usize,
    },
    /// A gzip file contained no member.
    MissingMember,
    /// A member ended before a required header, body, or trailer field.
    Truncated {
        /// Zero-based member index.
        member: usize,
        /// Record area that was incomplete.
        part: &'static str,
    },
    /// A member header violated RFC 1952.
    InvalidHeader {
        /// Zero-based member index.
        member: usize,
        /// Header validation diagnostic.
        reason: &'static str,
    },
    /// A member's optional header CRC did not match its header bytes.
    HeaderCrcMismatch {
        /// Zero-based member index.
        member: usize,
        /// CRC16 stored in the member.
        expected: u16,
        /// Low 16 bits of the computed CRC32.
        actual: u16,
    },
    /// The raw DEFLATE stream was invalid.
    InvalidDeflate {
        /// Zero-based member index.
        member: usize,
        /// Decoder diagnostic.
        message: String,
    },
    /// A member's decompressed-data CRC32 did not match its trailer.
    DataCrcMismatch {
        /// Zero-based member index.
        member: usize,
        /// CRC32 stored in the member trailer.
        expected: u32,
        /// CRC32 computed from this member's output.
        actual: u32,
    },
    /// A member's decompressed byte count did not match its trailer.
    DataSizeMismatch {
        /// Zero-based member index.
        member: usize,
        /// ISIZE stored in the member trailer.
        expected: u32,
        /// Decompressed member size modulo 2^32.
        actual: u32,
    },
    /// Bytes after a complete member were not another valid gzip member.
    TrailingData {
        /// Byte offset where the non-member tail starts.
        offset: usize,
    },
}

impl fmt::Display for GzipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CompressedSizeExceeded { limit, actual } => write!(
                f,
                "gzip archive is {actual} bytes, exceeding the {limit}-byte compressed limit"
            ),
            Self::DecompressedSizeExceeded { limit } => write!(
                f,
                "gzip output exceeds the {limit}-byte decompressed limit"
            ),
            Self::MissingMember => f.write_str("gzip file contains no member"),
            Self::Truncated { member, part } => {
                write!(f, "gzip member {member} has a truncated {part}")
            }
            Self::InvalidHeader { member, reason } => {
                write!(f, "gzip member {member} has an invalid header: {reason}")
            }
            Self::HeaderCrcMismatch {
                member,
                expected,
                actual,
            } => write!(
                f,
                "gzip member {member} header CRC mismatch: expected {expected:#06x}, got {actual:#06x}"
            ),
            Self::InvalidDeflate { member, message } => {
                write!(f, "gzip member {member} has invalid DEFLATE data: {message}")
            }
            Self::DataCrcMismatch {
                member,
                expected,
                actual,
            } => write!(
                f,
                "gzip member {member} data CRC mismatch: expected {expected:#010x}, got {actual:#010x}"
            ),
            Self::DataSizeMismatch {
                member,
                expected,
                actual,
            } => write!(
                f,
                "gzip member {member} size mismatch: expected {expected}, got {actual}"
            ),
            Self::TrailingData { offset } => {
                write!(f, "non-gzip data follows the last member at byte {offset}")
            }
        }
    }
}

impl std::error::Error for GzipError {}

impl From<GzipError> for std::io::Error {
    fn from(error: GzipError) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, error)
    }
}

/// Decode and validate a complete RFC 1952 gzip member series.
///
/// The compressed limit covers headers, DEFLATE payloads, trailers, and every
/// member in the supplied slice.  The decompressed limit is cumulative across
/// members.  Optional `FNAME` and `FCOMMENT` fields have no separate arbitrary
/// ceiling: the compressed limit bounds them.  An empty input, an incomplete
/// member, a bad header/body/trailer, or any non-member tail is rejected.
pub(crate) fn decode_gzip_members(bytes: &[u8], limits: GzipLimits) -> Result<Vec<u8>, GzipError> {
    if bytes.len() > limits.max_compressed_bytes {
        return Err(GzipError::CompressedSizeExceeded {
            limit: limits.max_compressed_bytes,
            actual: bytes.len(),
        });
    }
    if bytes.is_empty() {
        return Err(GzipError::MissingMember);
    }

    let initial_capacity = bytes.len().min(limits.max_decompressed_bytes);
    let mut decoded = Vec::with_capacity(initial_capacity);
    let mut cursor = 0usize;
    let mut member = 0usize;

    while cursor < bytes.len() {
        if member > 0
            && (bytes.len() - cursor < 2
                || bytes[cursor] != GZIP_ID1
                || bytes[cursor + 1] != GZIP_ID2)
        {
            return Err(GzipError::TrailingData { offset: cursor });
        }

        let body_start = parse_header(bytes, cursor, member)?;
        cursor = body_start;

        let mut inflater = Decompress::new(false);
        let mut crc = Hasher::new();
        let mut member_size = 0u32;
        let mut output_chunk = [0u8; OUTPUT_CHUNK_BYTES];

        loop {
            if cursor == bytes.len() {
                return Err(GzipError::Truncated {
                    member,
                    part: "DEFLATE body",
                });
            }

            let before_in = inflater.total_in();
            let before_out = inflater.total_out();
            let status = inflater
                .decompress(&bytes[cursor..], &mut output_chunk, FlushDecompress::None)
                .map_err(|error| GzipError::InvalidDeflate {
                    member,
                    message: error.to_string(),
                })?;
            let consumed = usize::try_from(inflater.total_in() - before_in).map_err(|_| {
                GzipError::InvalidDeflate {
                    member,
                    message: "compressed-byte counter overflow".to_string(),
                }
            })?;
            let produced = usize::try_from(inflater.total_out() - before_out).map_err(|_| {
                GzipError::InvalidDeflate {
                    member,
                    message: "decompressed-byte counter overflow".to_string(),
                }
            })?;

            cursor = cursor
                .checked_add(consumed)
                .ok_or_else(|| GzipError::InvalidDeflate {
                    member,
                    message: "compressed-byte position overflow".to_string(),
                })?;
            if decoded.len().saturating_add(produced) > limits.max_decompressed_bytes {
                return Err(GzipError::DecompressedSizeExceeded {
                    limit: limits.max_decompressed_bytes,
                });
            }
            crc.update(&output_chunk[..produced]);
            decoded.extend_from_slice(&output_chunk[..produced]);
            member_size = member_size.wrapping_add(produced as u32);

            if status == Status::StreamEnd {
                break;
            }
            if consumed == 0 && produced == 0 {
                return Err(GzipError::Truncated {
                    member,
                    part: "DEFLATE body",
                });
            }
        }

        let trailer_end = cursor
            .checked_add(TRAILER_BYTES)
            .filter(|end| *end <= bytes.len())
            .ok_or(GzipError::Truncated {
                member,
                part: "trailer",
            })?;
        let expected_crc = little_u32(&bytes[cursor..cursor + 4]);
        let expected_size = little_u32(&bytes[cursor + 4..trailer_end]);
        let actual_crc = crc.finalize();
        if expected_crc != actual_crc {
            return Err(GzipError::DataCrcMismatch {
                member,
                expected: expected_crc,
                actual: actual_crc,
            });
        }
        if expected_size != member_size {
            return Err(GzipError::DataSizeMismatch {
                member,
                expected: expected_size,
                actual: member_size,
            });
        }

        cursor = trailer_end;
        member += 1;
    }

    Ok(decoded)
}

fn parse_header(bytes: &[u8], start: usize, member: usize) -> Result<usize, GzipError> {
    let fixed_end = start
        .checked_add(FIXED_HEADER_BYTES)
        .filter(|end| *end <= bytes.len())
        .ok_or(GzipError::Truncated {
            member,
            part: "header",
        })?;
    if bytes[start] != GZIP_ID1 || bytes[start + 1] != GZIP_ID2 {
        return Err(GzipError::InvalidHeader {
            member,
            reason: "bad magic",
        });
    }
    if bytes[start + 2] != DEFLATE_METHOD {
        return Err(GzipError::InvalidHeader {
            member,
            reason: "unsupported compression method",
        });
    }
    let flags = bytes[start + 3];
    if flags & FLAG_RESERVED != 0 {
        return Err(GzipError::InvalidHeader {
            member,
            reason: "reserved flag bit is set",
        });
    }

    let mut cursor = fixed_end;
    if flags & FLAG_EXTRA != 0 {
        let length_end = cursor
            .checked_add(2)
            .filter(|end| *end <= bytes.len())
            .ok_or(GzipError::Truncated {
                member,
                part: "extra-field length",
            })?;
        let extra_len = usize::from(u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]));
        cursor = length_end
            .checked_add(extra_len)
            .filter(|end| *end <= bytes.len())
            .ok_or(GzipError::Truncated {
                member,
                part: "extra field",
            })?;
    }
    if flags & FLAG_NAME != 0 {
        cursor = nul_terminated_field_end(bytes, cursor, member, "file name")?;
    }
    if flags & FLAG_COMMENT != 0 {
        cursor = nul_terminated_field_end(bytes, cursor, member, "comment")?;
    }
    if flags & FLAG_HEADER_CRC != 0 {
        let crc_end = cursor
            .checked_add(2)
            .filter(|end| *end <= bytes.len())
            .ok_or(GzipError::Truncated {
                member,
                part: "header CRC",
            })?;
        let expected = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
        let actual = crc32fast::hash(&bytes[start..cursor]) as u16;
        if expected != actual {
            return Err(GzipError::HeaderCrcMismatch {
                member,
                expected,
                actual,
            });
        }
        cursor = crc_end;
    }
    Ok(cursor)
}

fn nul_terminated_field_end(
    bytes: &[u8],
    start: usize,
    member: usize,
    field: &'static str,
) -> Result<usize, GzipError> {
    let relative_end =
        bytes[start..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(GzipError::Truncated {
                member,
                part: field,
            })?;
    Ok(start + relative_end + 1)
}

fn little_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn member(
        payload: &[u8],
        extra: Option<&[u8]>,
        name: Option<&[u8]>,
        comment: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut flags = FLAG_HEADER_CRC;
        if extra.is_some() {
            flags |= FLAG_EXTRA;
        }
        if name.is_some() {
            flags |= FLAG_NAME;
        }
        if comment.is_some() {
            flags |= FLAG_COMMENT;
        }
        let mut archive = vec![
            GZIP_ID1,
            GZIP_ID2,
            DEFLATE_METHOD,
            flags,
            0,
            0,
            0,
            0,
            0,
            255,
        ];
        if let Some(extra) = extra {
            let length = u16::try_from(extra.len()).expect("test extra field fits XLEN");
            archive.extend_from_slice(&length.to_le_bytes());
            archive.extend_from_slice(extra);
        }
        if let Some(name) = name {
            archive.extend_from_slice(name);
            archive.push(0);
        }
        if let Some(comment) = comment {
            archive.extend_from_slice(comment);
            archive.push(0);
        }
        let header_crc = crc32fast::hash(&archive) as u16;
        archive.extend_from_slice(&header_crc.to_le_bytes());

        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).unwrap();
        archive.extend_from_slice(&encoder.finish().unwrap());
        archive.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
        archive.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        archive
    }

    fn limits_for(archive: &[u8], output: usize) -> GzipLimits {
        GzipLimits::new(archive.len(), output)
    }

    #[test]
    fn accepts_long_optional_strings_and_every_member() {
        let first = member(
            b"first",
            Some(&vec![b'e'; u16::MAX as usize]),
            Some(&vec![b'n'; 70_000]),
            Some(&vec![b'c'; 70_000]),
        );
        let second = member(b"", None, None, Some(&vec![b'x'; 70_000]));
        let third = member(b"third", None, None, None);
        let mut archive = first;
        archive.extend_from_slice(&second);
        archive.extend_from_slice(&third);

        assert_eq!(
            decode_gzip_members(&archive, limits_for(&archive, 10)).unwrap(),
            b"firstthird"
        );
    }

    #[test]
    fn rejects_corrupt_later_trailers_and_non_member_tails() {
        let first = member(b"first", None, None, None);
        let second_start = first.len();
        let mut archive = first;
        archive.extend_from_slice(&member(b"second", None, None, None));

        let mut bad_crc = archive.clone();
        let trailer = bad_crc.len() - TRAILER_BYTES;
        bad_crc[trailer] ^= 0x01;
        assert!(matches!(
            decode_gzip_members(&bad_crc, limits_for(&bad_crc, 11)),
            Err(GzipError::DataCrcMismatch { member: 1, .. })
        ));

        let mut bad_size = archive.clone();
        let isize = bad_size.len() - 4;
        bad_size[isize] ^= 0x01;
        assert!(matches!(
            decode_gzip_members(&bad_size, limits_for(&bad_size, 11)),
            Err(GzipError::DataSizeMismatch { member: 1, .. })
        ));

        let mut truncated = archive.clone();
        truncated.pop();
        assert!(matches!(
            decode_gzip_members(&truncated, limits_for(&truncated, 11)),
            Err(GzipError::Truncated { member: 1, .. })
        ));

        archive.extend_from_slice(b"junk");
        assert!(matches!(
            decode_gzip_members(&archive, limits_for(&archive, 11)),
            Err(GzipError::TrailingData { offset }) if offset > second_start
        ));
    }

    #[test]
    fn enforces_both_limits_at_the_exact_boundary() {
        let archive = member(b"payload", None, None, None);
        assert_eq!(
            decode_gzip_members(&archive, limits_for(&archive, 7)).unwrap(),
            b"payload"
        );
        assert!(matches!(
            decode_gzip_members(&archive, GzipLimits::new(archive.len() - 1, 7)),
            Err(GzipError::CompressedSizeExceeded { .. })
        ));
        assert!(matches!(
            decode_gzip_members(&archive, limits_for(&archive, 6)),
            Err(GzipError::DecompressedSizeExceeded { limit: 6 })
        ));
    }

    #[test]
    fn many_tiny_members_are_processed_as_one_linear_series() {
        let empty = member(b"", None, None, None);
        let mut archive = Vec::with_capacity(empty.len() * 10_000);
        for _ in 0..10_000 {
            archive.extend_from_slice(&empty);
        }
        assert_eq!(
            decode_gzip_members(&archive, limits_for(&archive, 0)).unwrap(),
            b""
        );
    }

    #[test]
    fn rejects_header_corruption_empty_input_and_partial_next_header() {
        assert_eq!(
            decode_gzip_members(&[], GzipLimits::new(1, 1)),
            Err(GzipError::MissingMember)
        );

        let mut bad_header_crc = member(b"payload", None, None, Some(b"comment"));
        bad_header_crc[10] ^= 1;
        assert!(matches!(
            decode_gzip_members(
                &bad_header_crc,
                limits_for(&bad_header_crc, b"payload".len())
            ),
            Err(GzipError::HeaderCrcMismatch { .. })
        ));

        let mut partial = member(b"payload", None, None, None);
        partial.push(GZIP_ID1);
        assert!(matches!(
            decode_gzip_members(&partial, limits_for(&partial, b"payload".len())),
            Err(GzipError::TrailingData { .. })
        ));
    }

    #[test]
    fn rejects_truncated_or_malformed_optional_headers() {
        let truncated_extra = [
            GZIP_ID1,
            GZIP_ID2,
            DEFLATE_METHOD,
            FLAG_EXTRA,
            0,
            0,
            0,
            0,
            0,
            255,
            5,
            0,
            b'a',
            b'b',
        ];
        assert!(matches!(
            decode_gzip_members(&truncated_extra, GzipLimits::new(truncated_extra.len(), 1)),
            Err(GzipError::Truncated {
                part: "extra field",
                ..
            })
        ));

        let mut unterminated_comment = vec![
            GZIP_ID1,
            GZIP_ID2,
            DEFLATE_METHOD,
            FLAG_COMMENT,
            0,
            0,
            0,
            0,
            0,
            255,
        ];
        unterminated_comment.extend_from_slice(b"not terminated");
        assert!(matches!(
            decode_gzip_members(
                &unterminated_comment,
                GzipLimits::new(unterminated_comment.len(), 1)
            ),
            Err(GzipError::Truncated {
                part: "comment",
                ..
            })
        ));

        let missing_header_crc = [
            GZIP_ID1,
            GZIP_ID2,
            DEFLATE_METHOD,
            FLAG_HEADER_CRC,
            0,
            0,
            0,
            0,
            0,
            255,
        ];
        assert!(matches!(
            decode_gzip_members(
                &missing_header_crc,
                GzipLimits::new(missing_header_crc.len(), 1)
            ),
            Err(GzipError::Truncated {
                part: "header CRC",
                ..
            })
        ));

        let mut reserved_flag = member(b"payload", None, None, None);
        reserved_flag[3] |= 0x20;
        assert!(matches!(
            decode_gzip_members(
                &reserved_flag,
                GzipLimits::new(reserved_flag.len(), b"payload".len())
            ),
            Err(GzipError::InvalidHeader { .. })
        ));
    }
}

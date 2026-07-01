//! Sans-I/O parsing and formatting primitives for format readers.
//!
//! This module is always present and crate-internal. It carries shared helpers
//! for parsers without depending on any GNSS-gated item.
//!
//! Invariant: a forgiving parse must push a typed [`Skip`] with a concrete
//! reason. It must never silently `continue`, and it must never fabricate a
//! default value for a malformed record. The API makes "skip with a reason" the
//! ordinary path.

#![allow(dead_code)]

/// Character-boundary-safe fixed-width column helpers.
pub(crate) mod columns;
/// Format-faithful numeric formatting helpers.
pub(crate) mod fmtnum;
/// KVN tokenizer and key/value field map.
pub(crate) mod kvn;
/// Logical-record grouping helpers.
pub(crate) mod records;
/// Whitespace-token scanner helpers.
pub(crate) mod tokens;

use crate::validate::FieldError;

/// Sans-I/O reader entry points for an in-memory format.
pub(crate) trait FormatReader {
    /// The value produced by this reader.
    type Output;

    /// Read a value from an in-memory UTF-8 string.
    fn read_str(&self, input: &str) -> Parsed<Self::Output>;

    /// Read a value from an in-memory byte slice.
    fn read_bytes(&self, input: &[u8]) -> Parsed<Self::Output>;
}

/// Sans-I/O writer entry point for an in-memory format.
pub(crate) trait FormatWriter {
    /// The value consumed by this writer.
    type Input;

    /// Write a value into a newly allocated string.
    fn write_string(&self, value: &Self::Input) -> String;
}

/// A parsed value plus diagnostics collected while reading it.
#[derive(Debug, Clone)]
pub struct Parsed<T> {
    /// The successfully parsed value.
    pub value: T,
    /// Non-fatal diagnostics collected while producing the value.
    pub diagnostics: Diagnostics,
}

impl<T> Parsed<T> {
    /// Build a parsed value with caller-supplied diagnostics.
    pub fn new(value: T, diagnostics: Diagnostics) -> Self {
        Self { value, diagnostics }
    }

    /// Build a parsed value with no diagnostics.
    pub fn clean(value: T) -> Self {
        Self::new(value, Diagnostics::new())
    }

    /// Borrow the parsed value.
    pub fn value(&self) -> &T {
        &self.value
    }

    /// Borrow the diagnostics.
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    /// Split this parsed result into its value and diagnostics.
    pub fn into_parts(self) -> (T, Diagnostics) {
        (self.value, self.diagnostics)
    }
}

/// Non-fatal parser diagnostics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diagnostics {
    /// Records skipped during a forgiving parse.
    pub skips: Vec<Skip>,
    /// Advisory warnings that did not prevent decoding.
    pub warnings: Vec<Warning>,
}

impl Diagnostics {
    /// Build an empty diagnostics set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return `true` when no skips or warnings are present.
    pub fn is_empty(&self) -> bool {
        self.skips.is_empty() && self.warnings.is_empty()
    }

    /// Add a skipped-record diagnostic.
    pub fn push_skip(&mut self, skip: Skip) {
        self.skips.push(skip);
    }

    /// Add an advisory warning.
    pub fn push_warning(&mut self, warning: Warning) {
        self.warnings.push(warning);
    }
}

/// A skipped record and the reason it was skipped.
#[derive(Debug, Clone, PartialEq)]
pub struct Skip {
    /// Where the skipped record came from.
    pub at: RecordRef,
    /// Why the record was skipped.
    pub reason: SkipReason,
}

/// Typed reasons a forgiving parser may skip a record.
#[derive(Debug, Clone, PartialEq)]
pub enum SkipReason {
    /// The record names a satellite that cannot be represented downstream.
    UnrepresentableSatellite,
    /// The record type is outside the reader's supported subset.
    UnsupportedRecordType(&'static str),
    /// A field failed typed validation.
    MalformedField(FieldError),
    /// The epoch lies outside the representable range for the target format.
    OutOfRangeEpoch,
    /// The record ended before all required fields were available.
    Truncated,
    /// The record names a unit outside the reader's supported set.
    UnsupportedUnit(String),
    /// A logical block is not modeled by this reader.
    UnknownBlock(String),
    /// The record is internally inconsistent.
    InconsistentRecord(&'static str),
}

/// An advisory warning attached to a record.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    /// Where the warning came from.
    pub at: RecordRef,
    /// The warning category.
    pub kind: WarningKind,
}

/// Advisory warning categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// A checksum did not match the record body.
    Checksum,
    /// A value was clamped to fit the target range.
    Clamped,
    /// A value lost precision or fidelity during conversion.
    Degraded,
    /// A declared count or mode did not match decoded records.
    Mismatch,
    /// Published validity intervals overlap.
    Overlap,
    /// A required-by-format metadata block was absent but recoverable.
    MissingMetadata,
}

/// A reference to a record in an input stream.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecordRef {
    /// The one-based input line number, when known.
    pub line: Option<usize>,
    /// The zero- or one-based logical record index chosen by the caller.
    pub record_index: Option<usize>,
    /// A raw satellite token, when known.
    pub satellite: Option<String>,
}

impl RecordRef {
    /// Build a record reference at a one-based line number.
    pub fn at_line(line: usize) -> Self {
        Self {
            line: Some(line),
            ..Self::default()
        }
    }

    /// Build a record reference at a logical record index.
    pub fn at_record(record_index: usize) -> Self {
        Self {
            record_index: Some(record_index),
            ..Self::default()
        }
    }

    /// Attach a raw satellite token to this reference.
    pub fn with_satellite(mut self, satellite: impl Into<String>) -> Self {
        self.satellite = Some(satellite.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_round_trips_diagnostics() {
        let mut diagnostics = Diagnostics::new();
        assert!(diagnostics.is_empty());

        let skip = Skip {
            at: RecordRef::at_line(3).with_satellite("G05"),
            reason: SkipReason::UnsupportedRecordType("DATA"),
        };
        diagnostics.push_skip(skip.clone());
        assert!(!diagnostics.is_empty());

        let warning = Warning {
            at: RecordRef::at_record(0),
            kind: WarningKind::Checksum,
        };
        diagnostics.push_warning(warning.clone());

        let parsed = Parsed::new(42, diagnostics.clone());
        assert_eq!(*parsed.value(), 42);
        assert_eq!(parsed.diagnostics(), &diagnostics);

        let (value, round_trip) = parsed.into_parts();
        assert_eq!(value, 42);
        assert_eq!(round_trip.skips, vec![skip]);
        assert_eq!(round_trip.warnings, vec![warning]);
    }

    #[test]
    fn clean_parsed_has_empty_diagnostics() {
        let parsed = Parsed::clean("ok");
        assert_eq!(parsed.value(), &"ok");
        assert!(parsed.diagnostics().is_empty());
    }

    #[test]
    fn malformed_field_wraps_field_error() {
        let field_error = FieldError::FloatParse {
            field: "epoch",
            value: "bad".to_string(),
        };
        let reason = SkipReason::MalformedField(field_error.clone());
        assert_eq!(reason, SkipReason::MalformedField(field_error));
    }
}

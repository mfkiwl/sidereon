//! Shared CCSDS Navigation Data Message epoch primitives.

use core::str::FromStr;

use crate::validate::{self, CivilSecondPolicy, FieldError};

/// CCSDS NDM calendar epoch split into lossless integer components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NdmEpoch {
    /// Gregorian calendar year.
    pub(crate) year: i32,
    /// Gregorian calendar month.
    pub(crate) month: u32,
    /// Gregorian calendar day of month.
    pub(crate) day: u32,
    /// Civil hour of day.
    pub(crate) hour: u32,
    /// Civil minute of hour.
    pub(crate) minute: u32,
    /// Civil second of minute.
    pub(crate) second: u32,
    /// Fractional second expressed in whole microseconds.
    pub(crate) microsecond: u32,
    /// Sub-microsecond remainder of the fractional second, in whole
    /// femtoseconds (0..1_000_000_000). Zero for the common 6-fractional-digit
    /// catalog epochs; nonzero only when the source spells out more than six
    /// fractional digits (kept exactly up to 15 digits, rounding on the 16th).
    pub(crate) femtosecond: u32,
}

impl NdmEpoch {
    /// Parse a CCSDS NDM epoch value using the supplied civil-second policy.
    ///
    /// Accepts `YYYY-MM-DDThh:mm:ss[.f...][Z]` with up to 15 fractional-second
    /// digits, preserved as whole microseconds plus a femtosecond remainder
    /// without rounding across the microsecond boundary.
    pub(crate) fn parse(
        text: &str,
        second_policy: CivilSecondPolicy,
    ) -> Result<NdmEpoch, FieldError> {
        let raw = text.trim();
        let text = raw.strip_suffix('Z').unwrap_or(raw);
        let (date, time) = text
            .split_once('T')
            .ok_or(FieldError::Missing { field: "epoch" })?;

        let mut date_parts = date.split('-');
        let year: i32 = epoch_int(date_parts.next())?;
        let month: u32 = epoch_int(date_parts.next())?;
        let day: u32 = epoch_int(date_parts.next())?;
        if let Some(extra) = date_parts.next() {
            return Err(FieldError::IntParse {
                field: "epoch",
                value: extra.to_string(),
            });
        }

        let mut time_parts = time.split(':');
        let hour: u32 = epoch_int(time_parts.next())?;
        let minute: u32 = epoch_int(time_parts.next())?;
        let sec_field = time_parts
            .next()
            .ok_or(FieldError::Missing { field: "epoch" })?;
        if let Some(extra) = time_parts.next() {
            return Err(FieldError::FloatParse {
                field: "epoch",
                value: extra.to_string(),
            });
        }

        let civil = validate::civil_datetime_with_femtosecond_policy(
            i64::from(year),
            i64::from(month),
            i64::from(day),
            i64::from(hour),
            i64::from(minute),
            sec_field,
            second_policy,
        )?;

        Ok(NdmEpoch {
            year: civil.year as i32,
            month: civil.month,
            day: civil.day,
            hour: civil.hour,
            minute: civil.minute,
            second: civil.second,
            microsecond: civil.microsecond,
            femtosecond: civil.femtosecond,
        })
    }

    /// Format this epoch as `YYYY-MM-DDThh:mm:ss.ffffff`, extending to the
    /// full 15 fractional digits only when a sub-microsecond remainder is
    /// present, so ordinary microsecond-precision epochs re-encode in their
    /// source form.
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_iso8601(&self) -> String {
        let fractional = if self.femtosecond == 0 {
            format!("{:06}", self.microsecond)
        } else {
            format!("{:06}{:09}", self.microsecond, self.femtosecond)
        };
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, fractional
        )
    }
}

/// Parse an epoch integer component, reporting missing or invalid fields.
fn epoch_int<T>(value: Option<&str>) -> Result<T, FieldError>
where
    T: FromStr,
{
    let value = value.ok_or(FieldError::Missing { field: "epoch" })?;
    validate::strict_int::<T>(value, "epoch")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_epoch_components_with_trailing_z() {
        assert_eq!(
            NdmEpoch::parse("2026-06-17T04:32:52.099296Z", CivilSecondPolicy::UtcLike).unwrap(),
            NdmEpoch {
                year: 2026,
                month: 6,
                day: 17,
                hour: 4,
                minute: 32,
                second: 52,
                microsecond: 99_296,
                femtosecond: 0,
            }
        );
    }

    #[test]
    fn six_digit_epoch_parses_with_zero_femtosecond_remainder() {
        // IR compatibility: a classic microsecond-precision epoch produces the
        // same component values as before the femtosecond extension, with the
        // new component strictly additive (zero).
        for text in [
            "2026-06-17T04:32:52.099296",
            "2026-06-17T04:32:52.5",
            "2026-06-17T04:32:52",
        ] {
            let epoch = NdmEpoch::parse(text, CivilSecondPolicy::UtcLike).unwrap();
            assert_eq!(epoch.femtosecond, 0, "{text} must carry no femtoseconds");
        }
        assert_eq!(
            NdmEpoch::parse("2026-06-17T04:32:52.5", CivilSecondPolicy::UtcLike)
                .unwrap()
                .microsecond,
            500_000,
        );
    }

    #[test]
    fn sub_microsecond_digits_split_without_rounding() {
        assert_eq!(
            NdmEpoch::parse("2026-06-17T04:32:52.9999995", CivilSecondPolicy::UtcLike).unwrap(),
            NdmEpoch {
                year: 2026,
                month: 6,
                day: 17,
                hour: 4,
                minute: 32,
                second: 52,
                microsecond: 999_999,
                femtosecond: 500_000_000,
            }
        );
    }

    #[test]
    fn sixteenth_fractional_digit_rounds_and_carries() {
        assert_eq!(
            NdmEpoch::parse(
                "2026-06-17T23:59:59.9999999999999995",
                CivilSecondPolicy::Continuous,
            )
            .unwrap(),
            NdmEpoch {
                year: 2026,
                month: 6,
                day: 18,
                hour: 0,
                minute: 0,
                second: 0,
                microsecond: 0,
                femtosecond: 0,
            }
        );
    }

    #[test]
    fn to_iso8601_round_trips_epoch_value() {
        let epoch =
            NdmEpoch::parse("2026-06-17T04:32:52.099296Z", CivilSecondPolicy::Continuous).unwrap();
        let encoded = epoch.to_iso8601();
        assert_eq!(encoded, "2026-06-17T04:32:52.099296");
        assert_eq!(
            NdmEpoch::parse(&encoded, CivilSecondPolicy::Continuous).unwrap(),
            epoch
        );
    }

    #[test]
    fn to_iso8601_round_trips_femtosecond_epoch_value() {
        let epoch =
            NdmEpoch::parse("2026-06-17T04:32:52.9999995", CivilSecondPolicy::Continuous).unwrap();
        let encoded = epoch.to_iso8601();
        assert_eq!(encoded, "2026-06-17T04:32:52.999999500000000");
        assert_eq!(
            NdmEpoch::parse(&encoded, CivilSecondPolicy::Continuous).unwrap(),
            epoch
        );
    }

    #[test]
    fn utc_like_accepts_leap_second_label() {
        assert_eq!(
            NdmEpoch::parse("2016-12-31T23:59:60.000000Z", CivilSecondPolicy::UtcLike,).unwrap(),
            NdmEpoch {
                year: 2016,
                month: 12,
                day: 31,
                hour: 23,
                minute: 59,
                second: 60,
                microsecond: 0,
                femtosecond: 0,
            }
        );
    }

    #[test]
    fn continuous_time_rejects_leap_second_label() {
        assert_eq!(
            NdmEpoch::parse("2016-12-31T23:59:60.000000Z", CivilSecondPolicy::Continuous,),
            Err(FieldError::InvalidCivilTime {
                field: "civil datetime",
                hour: 23,
                minute: 59,
                second: 60.0,
            })
        );
    }

    #[test]
    fn malformed_epoch_without_t_yields_field_error() {
        assert_eq!(
            NdmEpoch::parse("2026-06-17 04:32:52.099296Z", CivilSecondPolicy::UtcLike),
            Err(FieldError::Missing { field: "epoch" })
        );
    }

    #[test]
    fn surplus_date_or_time_segments_are_rejected() {
        assert!(
            NdmEpoch::parse("2026-06-17-05T04:32:52.099296", CivilSecondPolicy::UtcLike).is_err()
        );
        assert!(
            NdmEpoch::parse("2026-06-17T04:32:52.099296:00", CivilSecondPolicy::UtcLike).is_err()
        );
    }
}

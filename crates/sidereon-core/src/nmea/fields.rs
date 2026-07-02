use crate::validate::{self, FieldError};
use crate::{GnssSystem, Wgs84Geodetic};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NmeaTime {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub nanos: u32,
    pub decimals: u8,
}

impl NmeaTime {
    pub fn parse(token: &str) -> Result<Self, FieldError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(FieldError::Missing { field: "nmea time" });
        }
        let (whole, frac) = token.split_once('.').unwrap_or((token, ""));
        if whole.len() != 6 || !whole.bytes().all(|b| b.is_ascii_digit()) {
            return Err(FieldError::IntParse {
                field: "nmea time",
                value: token.to_string(),
            });
        }
        if frac.len() > 9 || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return Err(FieldError::IntParse {
                field: "nmea time fraction",
                value: token.to_string(),
            });
        }
        let hour = whole[0..2]
            .parse::<u8>()
            .map_err(|_| FieldError::IntParse {
                field: "nmea time hour",
                value: token.to_string(),
            })?;
        let minute = whole[2..4]
            .parse::<u8>()
            .map_err(|_| FieldError::IntParse {
                field: "nmea time minute",
                value: token.to_string(),
            })?;
        let second = whole[4..6]
            .parse::<u8>()
            .map_err(|_| FieldError::IntParse {
                field: "nmea time second",
                value: token.to_string(),
            })?;
        if hour > 23 || minute > 59 || second > 60 {
            return Err(FieldError::InvalidCivilTime {
                field: "nmea time",
                hour: i64::from(hour),
                minute: i64::from(minute),
                second: f64::from(second),
            });
        }
        let decimals = frac.len() as u8;
        let frac_value = if frac.is_empty() {
            0
        } else {
            frac.parse::<u32>().map_err(|_| FieldError::IntParse {
                field: "nmea time fraction",
                value: token.to_string(),
            })?
        };
        let nanos = frac_value * 10_u32.pow(9 - u32::from(decimals));
        Ok(Self {
            hour,
            minute,
            second,
            nanos,
            decimals,
        })
    }

    pub fn key(self) -> (u8, u8, u8, u32) {
        (self.hour, self.minute, self.second, self.nanos)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NmeaCoordinate {
    pub degrees: u16,
    pub minutes_scaled: u64,
    pub decimals: u8,
    pub negative: bool,
}

impl NmeaCoordinate {
    pub fn parse(value: &str, hemisphere: &str, is_latitude: bool) -> Result<Self, FieldError> {
        let value = value.trim();
        let hemisphere = hemisphere.trim();
        if value.is_empty() || hemisphere.is_empty() {
            return Err(FieldError::Missing {
                field: if is_latitude { "latitude" } else { "longitude" },
            });
        }
        let (negative, valid_hemisphere) = match hemisphere {
            "N" => (false, is_latitude),
            "S" => (true, is_latitude),
            "E" => (false, !is_latitude),
            "W" => (true, !is_latitude),
            _ => (false, false),
        };
        if !valid_hemisphere {
            return Err(FieldError::OutOfRange {
                field: "hemisphere",
                min: 0.0,
                max: 0.0,
                upper_inclusive: true,
            });
        }
        let degree_digits = if is_latitude { 2 } else { 3 };
        if value.len() < degree_digits + 2
            || !value[..degree_digits + 2]
                .bytes()
                .all(|b| b.is_ascii_digit())
        {
            return Err(FieldError::FloatParse {
                field: if is_latitude { "latitude" } else { "longitude" },
                value: value.to_string(),
            });
        }
        let degrees = value[..degree_digits]
            .parse::<u16>()
            .map_err(|_| FieldError::IntParse {
                field: "coordinate degrees",
                value: value.to_string(),
            })?;
        let minute_token = &value[degree_digits..];
        let (whole_minutes, minute_frac) =
            minute_token.split_once('.').unwrap_or((minute_token, ""));
        if whole_minutes.len() != 2
            || !whole_minutes.bytes().all(|b| b.is_ascii_digit())
            || minute_frac.len() > 9
            || !minute_frac.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(FieldError::FloatParse {
                field: "coordinate minutes",
                value: value.to_string(),
            });
        }
        let decimals = minute_frac.len() as u8;
        let scale = 10_u64.pow(u32::from(decimals));
        let minutes_whole = whole_minutes
            .parse::<u64>()
            .map_err(|_| FieldError::IntParse {
                field: "coordinate minutes",
                value: value.to_string(),
            })?;
        let frac_scaled = if minute_frac.is_empty() {
            0
        } else {
            minute_frac
                .parse::<u64>()
                .map_err(|_| FieldError::IntParse {
                    field: "coordinate minute fraction",
                    value: value.to_string(),
                })?
        };
        let minutes_scaled = minutes_whole * scale + frac_scaled;
        let degree_max = if is_latitude { 90 } else { 180 };
        if degrees > degree_max
            || minutes_whole > 59
            || (degrees == degree_max && minutes_scaled != 0)
        {
            return Err(FieldError::OutOfRange {
                field: if is_latitude { "latitude" } else { "longitude" },
                min: 0.0,
                max: f64::from(degree_max),
                upper_inclusive: true,
            });
        }
        Ok(Self {
            degrees,
            minutes_scaled,
            decimals,
            negative,
        })
    }

    pub fn from_degrees(
        degrees: f64,
        is_latitude: bool,
        decimals: u8,
    ) -> Result<Self, crate::nmea::NmeaError> {
        if !degrees.is_finite() || decimals > 9 {
            return Err(crate::nmea::NmeaError::InvalidInput {
                field: "coordinate",
                reason: "must be finite with at most 9 decimals",
            });
        }
        let max = if is_latitude { 90.0 } else { 180.0 };
        if degrees.abs() > max {
            return Err(crate::nmea::NmeaError::InvalidInput {
                field: "coordinate",
                reason: "out of range",
            });
        }
        let negative = degrees.is_sign_negative();
        let abs = degrees.abs();
        let mut whole_degrees = abs.floor() as u16;
        let scale = 10_u64.pow(u32::from(decimals));
        let minutes = (abs - f64::from(whole_degrees)) * 60.0;
        let mut minutes_scaled = round_half_away_from_zero(minutes * scale as f64) as u64;
        if minutes_scaled >= 60 * scale {
            whole_degrees += 1;
            minutes_scaled -= 60 * scale;
        }
        if f64::from(whole_degrees) > max {
            return Err(crate::nmea::NmeaError::InvalidInput {
                field: "coordinate",
                reason: "rounding exceeded coordinate bound",
            });
        }
        Ok(Self {
            degrees: whole_degrees,
            minutes_scaled,
            decimals,
            negative,
        })
    }

    pub fn degrees_f64(&self) -> f64 {
        let sign = if self.negative { -1.0 } else { 1.0 };
        let scale = 10_f64.powi(i32::from(self.decimals));
        sign * (f64::from(self.degrees) + (self.minutes_scaled as f64 / scale) / 60.0)
    }

    pub fn radians(&self) -> f64 {
        self.degrees_f64().to_radians()
    }
}

fn round_half_away_from_zero(value: f64) -> i64 {
    if value >= 0.0 {
        (value + 0.5).floor() as i64
    } else {
        (value - 0.5).ceil() as i64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NmeaDate {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NmeaTalker {
    System(GnssSystem),
    Combined,
    Other([u8; 2]),
}

impl NmeaTalker {
    pub fn parse(token: &str) -> Self {
        match token.as_bytes() {
            b"GP" => Self::System(GnssSystem::Gps),
            b"GL" => Self::System(GnssSystem::Glonass),
            b"GA" => Self::System(GnssSystem::Galileo),
            b"GB" | b"BD" => Self::System(GnssSystem::BeiDou),
            b"GQ" | b"QZ" => Self::System(GnssSystem::Qzss),
            b"GI" => Self::System(GnssSystem::Navic),
            b"GN" => Self::Combined,
            [a, b] => Self::Other([*a, *b]),
            _ => Self::Other([b'?', b'?']),
        }
    }

    pub fn code(self) -> Result<[u8; 2], crate::nmea::NmeaError> {
        match self {
            Self::System(GnssSystem::Gps) | Self::System(GnssSystem::Sbas) => Ok(*b"GP"),
            Self::System(GnssSystem::Glonass) => Ok(*b"GL"),
            Self::System(GnssSystem::Galileo) => Ok(*b"GA"),
            Self::System(GnssSystem::BeiDou) => Ok(*b"GB"),
            Self::System(GnssSystem::Qzss) => Ok(*b"GQ"),
            Self::System(GnssSystem::Navic) => Ok(*b"GI"),
            Self::Combined => Ok(*b"GN"),
            Self::Other(raw) if raw.iter().all(u8::is_ascii) => Ok(raw),
            Self::Other(_) => Err(crate::nmea::NmeaError::InvalidInput {
                field: "talker",
                reason: "must be ASCII",
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GgaQuality {
    Invalid,
    GpsSps,
    Differential,
    Pps,
    RtkFixed,
    RtkFloat,
    Estimated,
    Manual,
    Simulator,
    Other(u8),
}

impl GgaQuality {
    pub fn parse(token: &str) -> Result<Self, FieldError> {
        let value = validate::strict_int::<u8>(token, "gga quality")?;
        Ok(match value {
            0 => Self::Invalid,
            1 => Self::GpsSps,
            2 => Self::Differential,
            3 => Self::Pps,
            4 => Self::RtkFixed,
            5 => Self::RtkFloat,
            6 => Self::Estimated,
            7 => Self::Manual,
            8 => Self::Simulator,
            other => Self::Other(other),
        })
    }

    pub fn value(self) -> u8 {
        match self {
            Self::Invalid => 0,
            Self::GpsSps => 1,
            Self::Differential => 2,
            Self::Pps => 3,
            Self::RtkFixed => 4,
            Self::RtkFloat => 5,
            Self::Estimated => 6,
            Self::Manual => 7,
            Self::Simulator => 8,
            Self::Other(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gga {
    pub time: Option<NmeaTime>,
    pub latitude: Option<NmeaCoordinate>,
    pub longitude: Option<NmeaCoordinate>,
    pub quality: Option<GgaQuality>,
    pub satellites_used: Option<u8>,
    pub hdop: Option<f64>,
    pub altitude_msl_m: Option<f64>,
    pub geoid_separation_m: Option<f64>,
    pub differential_age_s: Option<f64>,
    pub differential_station_id: Option<u16>,
}

impl Gga {
    pub fn vrs_position(
        position: Wgs84Geodetic,
        time: NmeaTime,
        quality: GgaQuality,
        satellites_used: u8,
        hdop: f64,
        coordinate_decimals: u8,
    ) -> Result<Self, crate::nmea::NmeaError> {
        Ok(Self {
            time: Some(time),
            latitude: Some(NmeaCoordinate::from_degrees(
                position.lat_rad.to_degrees(),
                true,
                coordinate_decimals,
            )?),
            longitude: Some(NmeaCoordinate::from_degrees(
                position.lon_rad.to_degrees(),
                false,
                coordinate_decimals,
            )?),
            quality: Some(quality),
            satellites_used: Some(satellites_used),
            hdop: Some(hdop),
            altitude_msl_m: Some(position.height_m),
            geoid_separation_m: Some(0.0),
            differential_age_s: None,
            differential_station_id: None,
        })
    }
}

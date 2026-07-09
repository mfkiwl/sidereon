use crate::frequencies::CarrierBand;
use crate::validate::{self, FieldError};
use crate::{GnssSatelliteId, GnssSystem, Wgs84Geodetic};
use std::time::Duration;

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

    pub fn from_seconds_of_day_floor_centis(seconds: f64) -> Result<Self, crate::nmea::NmeaError> {
        if !seconds.is_finite() || !(0.0..86_400.0).contains(&seconds) {
            return Err(crate::nmea::NmeaError::InvalidInput {
                field: "time",
                reason: "must be finite and in [0, 86400)",
            });
        }
        let whole = seconds.floor() as u32;
        let fractional = (seconds - f64::from(whole)).clamp(0.0, 1.0);
        let centis = (Duration::from_secs_f64(fractional).as_nanos() / 10_000_000).min(99) as u32;
        Ok(Self {
            hour: (whole / 3600) as u8,
            minute: ((whole % 3600) / 60) as u8,
            second: (whole % 60) as u8,
            nanos: centis * 10_000_000,
            decimals: 2,
        })
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

impl NmeaDate {
    pub fn parse_rmc(token: &str) -> Result<Self, FieldError> {
        let token = token.trim();
        if token.len() != 6 || !token.bytes().all(|b| b.is_ascii_digit()) {
            return Err(FieldError::IntParse {
                field: "nmea date",
                value: token.to_string(),
            });
        }
        let day = parse_u8(&token[0..2], "nmea date day")?;
        let month = parse_u8(&token[2..4], "nmea date month")?;
        let yy = parse_u8(&token[4..6], "nmea date year")?;
        let year = if yy >= 80 {
            1900 + u16::from(yy)
        } else {
            2000 + u16::from(yy)
        };
        Self::new(year, month, day)
    }

    pub fn new(year: u16, month: u8, day: u8) -> Result<Self, FieldError> {
        let max_day = crate::astro::time::civil::days_in_month(i64::from(year), i64::from(month));
        if max_day == 0 || day == 0 || i64::from(day) > max_day {
            return Err(FieldError::InvalidCivilDate {
                field: "nmea date",
                year: i64::from(year),
                month: i64::from(month),
                day: i64::from(day),
            });
        }
        Ok(Self { year, month, day })
    }

    pub fn next_day(self) -> Self {
        let max_day =
            crate::astro::time::civil::days_in_month(i64::from(self.year), i64::from(self.month))
                as u8;
        if self.day < max_day {
            Self {
                day: self.day + 1,
                ..self
            }
        } else if self.month < 12 {
            Self {
                month: self.month + 1,
                day: 1,
                ..self
            }
        } else {
            Self {
                year: self.year + 1,
                month: 1,
                day: 1,
            }
        }
    }
}

fn parse_u8(token: &str, field: &'static str) -> Result<u8, FieldError> {
    token.parse::<u8>().map_err(|_| FieldError::IntParse {
        field,
        value: token.to_string(),
    })
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
            _ => Self::Other(*b"??"),
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

    pub fn system(self) -> Option<GnssSystem> {
        match self {
            Self::System(system) => Some(system),
            Self::Combined | Self::Other(_) => None,
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
        if !hdop.is_finite() || hdop < 0.0 {
            return Err(crate::nmea::NmeaError::InvalidInput {
                field: "hdop",
                reason: "must be finite and non-negative",
            });
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NmeaSatNumber {
    pub raw: u16,
    pub resolved: Option<GnssSatelliteId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NmeaSignalId {
    pub system: Option<GnssSystem>,
    pub id: u8,
}

impl NmeaSignalId {
    pub fn carrier_band(&self) -> Option<CarrierBand> {
        let system = self.system?;
        match system {
            GnssSystem::Gps | GnssSystem::Sbas => match self.id {
                1..=3 => Some(CarrierBand::L1),
                4..=6 => Some(CarrierBand::L2),
                7 | 8 => Some(CarrierBand::L5),
                _ => None,
            },
            GnssSystem::Glonass => match self.id {
                1 | 2 => Some(CarrierBand::G1),
                3 | 4 => Some(CarrierBand::G2),
                _ => None,
            },
            GnssSystem::Galileo => match self.id {
                1 => Some(CarrierBand::E5a),
                2 => Some(CarrierBand::E5b),
                3 => Some(CarrierBand::E5),
                4 | 5 => Some(CarrierBand::E6),
                6 | 7 => Some(CarrierBand::E1),
                _ => None,
            },
            GnssSystem::BeiDou => match self.id {
                1 | 2 => Some(CarrierBand::B1i),
                3 | 4 => Some(CarrierBand::B1c),
                5 => Some(CarrierBand::B2a),
                6 => Some(CarrierBand::B2b),
                7 => Some(CarrierBand::B2),
                8 | 9 => Some(CarrierBand::B3i),
                _ => None,
            },
            GnssSystem::Qzss => match self.id {
                1..=4 => Some(CarrierBand::L1),
                5 | 6 => Some(CarrierBand::L2),
                7 | 8 => Some(CarrierBand::L5),
                _ => None,
            },
            GnssSystem::Navic => match self.id {
                1 | 3 => Some(CarrierBand::L5),
                _ => None,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RmcStatus {
    Valid,
    Warning,
    Other(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsaSelectionMode {
    Manual,
    Automatic,
    Other(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsaFixMode {
    None,
    TwoD,
    ThreeD,
    Other(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rmc {
    pub time: Option<NmeaTime>,
    pub status: Option<RmcStatus>,
    pub latitude: Option<NmeaCoordinate>,
    pub longitude: Option<NmeaCoordinate>,
    pub speed_over_ground_kn: Option<f64>,
    pub course_over_ground_deg: Option<f64>,
    pub date: Option<NmeaDate>,
    pub magnetic_variation_deg: Option<f64>,
    pub faa_mode: Option<char>,
    pub navigational_status: Option<char>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gsa {
    pub selection_mode: Option<GsaSelectionMode>,
    pub fix_mode: Option<GsaFixMode>,
    pub satellites: Vec<NmeaSatNumber>,
    pub pdop: Option<f64>,
    pub hdop: Option<f64>,
    pub vdop: Option<f64>,
    pub system_id: Option<u8>,
    pub system: Option<GnssSystem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GsvSatellite {
    pub sat_number: Option<NmeaSatNumber>,
    pub elevation_deg: Option<i16>,
    pub azimuth_deg: Option<u16>,
    pub cn0_db_hz: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gsv {
    pub total_messages: u8,
    pub message_number: u8,
    pub satellites_in_view: Option<u16>,
    pub satellites: Vec<GsvSatellite>,
    pub signal: Option<NmeaSignalId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gst {
    pub time: Option<NmeaTime>,
    pub rms_range_residual_m: Option<f64>,
    pub semi_major_error_m: Option<f64>,
    pub semi_minor_error_m: Option<f64>,
    pub orientation_deg: Option<f64>,
    pub latitude_sigma_m: Option<f64>,
    pub longitude_sigma_m: Option<f64>,
    pub altitude_sigma_m: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vtg {
    pub course_true_deg: Option<f64>,
    pub course_magnetic_deg: Option<f64>,
    pub speed_kn: Option<f64>,
    pub speed_kmh: Option<f64>,
    pub faa_mode: Option<char>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gll {
    pub latitude: Option<NmeaCoordinate>,
    pub longitude: Option<NmeaCoordinate>,
    pub time: Option<NmeaTime>,
    pub status: Option<RmcStatus>,
    pub faa_mode: Option<char>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Zda {
    pub time: Option<NmeaTime>,
    pub date: Option<NmeaDate>,
    pub local_zone_hours: Option<i8>,
    pub local_zone_minutes: Option<u8>,
}

pub(crate) fn resolve_sat_number(context: Option<GnssSystem>, raw: u16) -> Option<GnssSatelliteId> {
    let candidate = match context {
        Some(GnssSystem::Gps) => match raw {
            1..=32 => Some((GnssSystem::Gps, raw)),
            33..=64 => Some((GnssSystem::Sbas, raw - 13)),
            _ => None,
        },
        Some(GnssSystem::Glonass) => match raw {
            65..=99 => Some((GnssSystem::Glonass, raw - 64)),
            1..=35 => Some((GnssSystem::Glonass, raw)),
            _ => None,
        },
        Some(GnssSystem::Galileo) => match raw {
            1..=36 => Some((GnssSystem::Galileo, raw)),
            _ => None,
        },
        Some(GnssSystem::BeiDou) => match raw {
            1..=64 => Some((GnssSystem::BeiDou, raw)),
            _ => None,
        },
        Some(GnssSystem::Qzss) => match raw {
            1..=10 => Some((GnssSystem::Qzss, raw)),
            193..=202 => Some((GnssSystem::Qzss, raw - 192)),
            _ => None,
        },
        Some(GnssSystem::Navic) => match raw {
            1..=15 => Some((GnssSystem::Navic, raw)),
            _ => None,
        },
        Some(GnssSystem::Sbas) => match raw {
            33..=64 => Some((GnssSystem::Sbas, raw - 13)),
            120..=158 => Some((GnssSystem::Sbas, raw - 100)),
            _ => None,
        },
        None => match raw {
            1..=32 => Some((GnssSystem::Gps, raw)),
            33..=64 => Some((GnssSystem::Sbas, raw - 13)),
            65..=99 => Some((GnssSystem::Glonass, raw - 64)),
            193..=202 => Some((GnssSystem::Qzss, raw - 192)),
            _ => None,
        },
    }?;
    let prn = u8::try_from(candidate.1).ok()?;
    GnssSatelliteId::new(candidate.0, prn).ok()
}

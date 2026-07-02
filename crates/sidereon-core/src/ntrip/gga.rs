use crate::nmea::{self, Gga, GgaQuality, NmeaTalker, NmeaTime};
use crate::{Error, Result, Wgs84Geodetic};

#[derive(Clone, Debug, PartialEq)]
pub struct GgaPosition {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub height_m: f64,
    pub fix_quality: u8,
    pub num_satellites: u8,
    pub hdop: f64,
}

impl Default for GgaPosition {
    fn default() -> Self {
        Self {
            lat_deg: 0.0,
            lon_deg: 0.0,
            height_m: 0.0,
            fix_quality: 1,
            num_satellites: 10,
            hdop: 1.0,
        }
    }
}

pub fn format_gga(position: &GgaPosition, utc_seconds_of_day: f64) -> Result<Vec<u8>> {
    validate(position, utc_seconds_of_day)?;
    let geodetic = Wgs84Geodetic::new(
        position.lat_deg.to_radians(),
        position.lon_deg.to_radians(),
        position.height_m,
    )
    .map_err(|err| Error::InvalidInput(err.to_string()))?;
    let mut gga = Gga::vrs_position(
        geodetic,
        format_time(utc_seconds_of_day),
        quality(position.fix_quality),
        position.num_satellites,
        position.hdop,
        7,
    )
    .map_err(|err| Error::InvalidInput(err.to_string()))?;
    gga.geoid_separation_m = None;
    nmea::write_gga(NmeaTalker::System(crate::GnssSystem::Gps), &gga)
        .map(|sentence| sentence.into_bytes())
        .map_err(|err| Error::InvalidInput(err.to_string()))
}

fn validate(position: &GgaPosition, utc_seconds_of_day: f64) -> Result<()> {
    if !position.lat_deg.is_finite()
        || !position.lon_deg.is_finite()
        || !position.height_m.is_finite()
        || !position.hdop.is_finite()
        || !utc_seconds_of_day.is_finite()
    {
        return Err(Error::InvalidInput("GGA inputs must be finite".into()));
    }
    if !(-90.0..=90.0).contains(&position.lat_deg) {
        return Err(Error::InvalidInput("GGA latitude outside [-90, 90]".into()));
    }
    if !(-180.0..=180.0).contains(&position.lon_deg) {
        return Err(Error::InvalidInput(
            "GGA longitude outside [-180, 180]".into(),
        ));
    }
    if position.hdop < 0.0 {
        return Err(Error::InvalidInput("GGA HDOP must be non-negative".into()));
    }
    if !(0.0..86400.0).contains(&utc_seconds_of_day) {
        return Err(Error::InvalidInput("GGA time must be in [0, 86400)".into()));
    }
    Ok(())
}

fn format_time(seconds: f64) -> NmeaTime {
    let centis = (seconds * 100.0).floor() as u32;
    let whole = centis / 100;
    NmeaTime {
        hour: (whole / 3600) as u8,
        minute: ((whole % 3600) / 60) as u8,
        second: (whole % 60) as u8,
        nanos: (centis % 100) * 10_000_000,
        decimals: 2,
    }
}

fn quality(value: u8) -> GgaQuality {
    match value {
        0 => GgaQuality::Invalid,
        1 => GgaQuality::GpsSps,
        2 => GgaQuality::Differential,
        3 => GgaQuality::Pps,
        4 => GgaQuality::RtkFixed,
        5 => GgaQuality::RtkFloat,
        6 => GgaQuality::Estimated,
        7 => GgaQuality::Manual,
        8 => GgaQuality::Simulator,
        other => GgaQuality::Other(other),
    }
}

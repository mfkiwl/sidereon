use crate::{Error, Result};

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
    let time = format_time(utc_seconds_of_day);
    let (lat, ns) = format_coord(position.lat_deg, 2, 'N', 'S');
    let (lon, ew) = format_coord(position.lon_deg, 3, 'E', 'W');
    let body = format!(
        "GPGGA,{time},{lat},{ns},{lon},{ew},{},{:02},{:.1},{:.3},M,,M,,",
        position.fix_quality, position.num_satellites, position.hdop, position.height_m
    );
    let checksum = body.bytes().fold(0u8, |acc, b| acc ^ b);
    Ok(format!("${body}*{checksum:02X}\r\n").into_bytes())
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

fn format_time(seconds: f64) -> String {
    let centis = (seconds * 100.0).floor() as u32;
    let whole = centis / 100;
    let cs = centis % 100;
    let h = whole / 3600;
    let m = (whole % 3600) / 60;
    let s = whole % 60;
    format!("{h:02}{m:02}{s:02}.{cs:02}")
}

fn format_coord(value: f64, degree_width: usize, pos: char, neg: char) -> (String, char) {
    let hemi = if value.is_sign_negative() { neg } else { pos };
    let abs = value.abs();
    let mut deg = abs.floor() as u32;
    let minutes = (abs - f64::from(deg)) * 60.0;
    let mut minute_units = (minutes * 10_000_000.0 + 0.5).floor() as u64;
    if minute_units >= 600_000_000 {
        deg += 1;
        minute_units -= 600_000_000;
    }
    let whole_min = minute_units / 10_000_000;
    let frac = minute_units % 10_000_000;
    (
        format!("{deg:0degree_width$}{whole_min:02}.{frac:07}"),
        hemi,
    )
}

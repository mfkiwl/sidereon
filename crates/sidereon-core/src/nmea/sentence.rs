use crate::format::{Diagnostics, Parsed, RecordRef, Warning, WarningKind};
use crate::validate::{self, FieldError};

use super::{Gga, GgaQuality, NmeaCoordinate, NmeaError, NmeaTalker, NmeaTime};

#[derive(Debug, Clone, PartialEq)]
pub struct NmeaSentence {
    pub talker: NmeaTalker,
    pub body: NmeaBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NmeaBody {
    Gga(Gga),
}

pub(crate) struct FramedSentence<'a> {
    pub delimiter: u8,
    pub body: &'a str,
    pub diagnostics: Diagnostics,
}

pub(crate) fn checksum_body(body: &str) -> u8 {
    body.bytes().fold(0, |acc, byte| acc ^ byte)
}

pub(crate) fn frame_sentence(line: &str) -> Result<FramedSentence<'_>, NmeaError> {
    let mut diagnostics = Diagnostics::new();
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let start =
        trimmed
            .bytes()
            .position(|b| b == b'$' || b == b'!')
            .ok_or(NmeaError::NotFramed {
                reason: "no NMEA start delimiter",
            })?;
    if start > 0 || !line[..start].trim().is_empty() {
        diagnostics.push_warning(Warning {
            at: RecordRef::default(),
            kind: WarningKind::Mismatch,
        });
    }
    let sentence = &trimmed[start..];
    if sentence.len() > 1024 {
        return Err(NmeaError::NotFramed {
            reason: "sentence over length cap",
        });
    }
    if !sentence.is_ascii() {
        return Err(NmeaError::NotFramed {
            reason: "non-ASCII byte",
        });
    }
    let delimiter = sentence.as_bytes()[0];
    let rest = &sentence[1..];
    let (body, checksum) = if let Some(star) = rest.find('*') {
        let checksum_token = rest.get(star + 1..star + 3).unwrap_or("");
        if checksum_token.len() != 2 || !checksum_token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(NmeaError::NotFramed {
                reason: "malformed checksum",
            });
        }
        let trailing = &rest[star + 3..];
        if !trailing
            .bytes()
            .all(|b| b == b' ' || b == b'\r' || b == b'\n')
        {
            diagnostics.push_warning(Warning {
                at: RecordRef::default(),
                kind: WarningKind::Mismatch,
            });
        }
        let stated = u8::from_str_radix(checksum_token, 16).map_err(|_| NmeaError::NotFramed {
            reason: "malformed checksum",
        })?;
        (&rest[..star], Some(stated))
    } else {
        diagnostics.push_warning(Warning {
            at: RecordRef::default(),
            kind: WarningKind::MissingMetadata,
        });
        (rest.trim_end(), None)
    };
    if let Some(stated) = checksum {
        let computed = checksum_body(body);
        if computed != stated {
            return Err(NmeaError::ChecksumMismatch { computed, stated });
        }
    }
    Ok(FramedSentence {
        delimiter,
        body,
        diagnostics,
    })
}

pub(crate) fn parse_framed(framed: FramedSentence<'_>) -> Result<Parsed<NmeaSentence>, NmeaError> {
    if framed.delimiter == b'!' {
        return Err(NmeaError::UnsupportedType {
            address: "encapsulated".to_string(),
        });
    }
    let mut parts = framed.body.split(',');
    let address = parts.next().unwrap_or_default();
    if address.starts_with('P') {
        return Err(NmeaError::Proprietary {
            address: address.to_string(),
        });
    }
    if address.len() != 5 {
        return Err(NmeaError::UnsupportedType {
            address: address.to_string(),
        });
    }
    let talker = NmeaTalker::parse(&address[..2]);
    let sentence_type = &address[2..];
    let fields: Vec<&str> = parts.collect();
    let body = match sentence_type {
        "GGA" => NmeaBody::Gga(parse_gga(&fields)?),
        _ => {
            return Err(NmeaError::UnsupportedType {
                address: address.to_string(),
            })
        }
    };
    Ok(Parsed::new(
        NmeaSentence { talker, body },
        framed.diagnostics,
    ))
}

fn parse_gga(fields: &[&str]) -> Result<Gga, FieldError> {
    Ok(Gga {
        time: parse_opt_time(fields.first().copied())?,
        latitude: parse_opt_coordinate(fields.get(1).copied(), fields.get(2).copied(), true)?,
        longitude: parse_opt_coordinate(fields.get(3).copied(), fields.get(4).copied(), false)?,
        quality: parse_opt_quality(fields.get(5).copied())?,
        satellites_used: parse_opt_u8_range(fields.get(6).copied(), "satellites used", 0, 99)?,
        hdop: parse_opt_f64(fields.get(7).copied(), "hdop")?,
        altitude_msl_m: parse_opt_unit_f64(
            fields.get(8).copied(),
            fields.get(9).copied(),
            "altitude msl",
        )?,
        geoid_separation_m: parse_opt_unit_f64(
            fields.get(10).copied(),
            fields.get(11).copied(),
            "geoid separation",
        )?,
        differential_age_s: parse_opt_f64(fields.get(12).copied(), "differential age")?,
        differential_station_id: parse_opt_u16_range(
            fields.get(13).copied(),
            "differential station id",
            0,
            9999,
        )?,
    })
}

fn parse_opt_time(token: Option<&str>) -> Result<Option<NmeaTime>, FieldError> {
    match token.unwrap_or("").trim() {
        "" => Ok(None),
        token => NmeaTime::parse(token).map(Some),
    }
}

fn parse_opt_coordinate(
    value: Option<&str>,
    hemisphere: Option<&str>,
    is_latitude: bool,
) -> Result<Option<NmeaCoordinate>, FieldError> {
    let value = value.unwrap_or("").trim();
    let hemisphere = hemisphere.unwrap_or("").trim();
    match (value.is_empty(), hemisphere.is_empty()) {
        (true, true) => Ok(None),
        (false, false) => NmeaCoordinate::parse(value, hemisphere, is_latitude).map(Some),
        _ => Err(FieldError::Missing {
            field: if is_latitude {
                "latitude pair"
            } else {
                "longitude pair"
            },
        }),
    }
}

fn parse_opt_quality(token: Option<&str>) -> Result<Option<GgaQuality>, FieldError> {
    match token.unwrap_or("").trim() {
        "" => Ok(None),
        token => GgaQuality::parse(token).map(Some),
    }
}

fn parse_opt_f64(token: Option<&str>, field: &'static str) -> Result<Option<f64>, FieldError> {
    match token.unwrap_or("").trim() {
        "" => Ok(None),
        token => validate::strict_f64(token, field).map(Some),
    }
}

fn parse_opt_unit_f64(
    value: Option<&str>,
    unit: Option<&str>,
    field: &'static str,
) -> Result<Option<f64>, FieldError> {
    let value = value.unwrap_or("").trim();
    let unit = unit.unwrap_or("").trim();
    if value.is_empty() {
        if unit.is_empty() {
            return Ok(None);
        }
        return Err(FieldError::Missing { field });
    }
    if unit != "M" {
        return Err(FieldError::OutOfRange {
            field: "unit",
            min: 0.0,
            max: 0.0,
            upper_inclusive: true,
        });
    }
    validate::strict_f64(value, field).map(Some)
}

fn parse_opt_u8_range(
    token: Option<&str>,
    field: &'static str,
    min: u8,
    max: u8,
) -> Result<Option<u8>, FieldError> {
    match token.unwrap_or("").trim() {
        "" => Ok(None),
        token => {
            let value = validate::strict_int::<u8>(token, field)?;
            if value < min || value > max {
                Err(FieldError::OutOfRange {
                    field,
                    min: f64::from(min),
                    max: f64::from(max),
                    upper_inclusive: true,
                })
            } else {
                Ok(Some(value))
            }
        }
    }
}

fn parse_opt_u16_range(
    token: Option<&str>,
    field: &'static str,
    min: u16,
    max: u16,
) -> Result<Option<u16>, FieldError> {
    match token.unwrap_or("").trim() {
        "" => Ok(None),
        token => {
            let value = validate::strict_int::<u16>(token, field)?;
            if value < min || value > max {
                Err(FieldError::OutOfRange {
                    field,
                    min: f64::from(min),
                    max: f64::from(max),
                    upper_inclusive: true,
                })
            } else {
                Ok(Some(value))
            }
        }
    }
}

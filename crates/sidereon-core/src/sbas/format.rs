use crate::astro::time::gnss::{seconds_of_week_from_calendar, week_from_calendar};
use crate::astro::time::model::{GnssWeekTow, TimeScale};
use crate::error::{Error, Result};
use crate::id::GnssSatelliteId;

use super::message::SbasWireForm;
use super::store::sbas_prn_to_sat;

#[derive(Clone, Debug, PartialEq)]
pub struct SbasLogBlock {
    pub satellite_id: GnssSatelliteId,
    pub epoch: GnssWeekTow,
    pub form: SbasWireForm,
    pub bytes: Vec<u8>,
}

pub fn parse_ems_lines(text: &str) -> Result<Vec<SbasLogBlock>> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(block) = parse_ems_line(line)? {
            out.push(block);
        }
    }
    Ok(out)
}

pub fn parse_rtklib_lines(text: &str) -> Result<Vec<SbasLogBlock>> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(block) = parse_rtklib_line(line)? {
            out.push(block);
        }
    }
    Ok(out)
}

fn parse_ems_line(line: &str) -> Result<Option<SbasLogBlock>> {
    let parts: Vec<&str> = line
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() < 8 {
        return Ok(None);
    }
    let Some(hex) = parts.last().copied().filter(|s| looks_hex(s)) else {
        return Ok(None);
    };
    let Some(prn) = parse_u16(parts[0]) else {
        return Ok(None);
    };
    let Some(satellite_id) = sbas_prn_to_sat(prn) else {
        return Ok(None);
    };
    let Some(year) = parse_i64(parts[1]) else {
        return Ok(None);
    };
    let Some(month) = parse_i64(parts[2]) else {
        return Ok(None);
    };
    let Some(day) = parse_i64(parts[3]) else {
        return Ok(None);
    };
    let Some(hour) = parse_i64(parts[4]) else {
        return Ok(None);
    };
    let Some(minute) = parse_i64(parts[5]) else {
        return Ok(None);
    };
    let Some(second) = parse_i64(parts[6]) else {
        return Ok(None);
    };
    let year = if year < 100 { 2000 + year } else { year };
    let Some(week) = week_from_calendar(TimeScale::Gpst, year, month, day) else {
        return Ok(None);
    };
    let tow_s = seconds_of_week_from_calendar(year, month, day, hour, minute, second);
    let epoch = GnssWeekTow::new(TimeScale::Gpst, week, tow_s)
        .map_err(|e| Error::Parse(format!("invalid SBAS EMS epoch: {e}")))?;
    let (form, bytes) = decode_hex_block(hex)?;
    Ok(Some(SbasLogBlock {
        satellite_id,
        epoch,
        form,
        bytes,
    }))
}

fn parse_rtklib_line(line: &str) -> Result<Option<SbasLogBlock>> {
    let Some((head, hex)) = line.split_once(':') else {
        return Ok(None);
    };
    if !looks_hex(hex.trim()) {
        return Ok(None);
    }
    let fields: Vec<&str> = head.split_whitespace().collect();
    if fields.len() < 4 {
        return Ok(None);
    }
    let Some(week) = parse_u32(fields[0]) else {
        return Ok(None);
    };
    let Some(tow_s) = parse_f64(fields[1]) else {
        return Ok(None);
    };
    let Some(prn) = parse_u16(fields[2]) else {
        return Ok(None);
    };
    let Some(satellite_id) = sbas_prn_to_sat(prn) else {
        return Ok(None);
    };
    let epoch = GnssWeekTow::new(TimeScale::Gpst, week, tow_s)
        .map_err(|e| Error::Parse(format!("invalid SBAS RTKLIB epoch: {e}")))?;
    let (_, bytes) = decode_hex_block(hex.trim())?;
    Ok(Some(SbasLogBlock {
        satellite_id,
        epoch,
        form: SbasWireForm::Body226,
        bytes,
    }))
}

fn decode_hex_block(hex: &str) -> Result<(SbasWireForm, Vec<u8>)> {
    let mut clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if !clean.len().is_multiple_of(2) {
        clean.push('0');
    }
    let mut bytes = Vec::with_capacity(clean.len() / 2);
    for idx in (0..clean.len()).step_by(2) {
        let byte = u8::from_str_radix(&clean[idx..idx + 2], 16)
            .map_err(|e| Error::Parse(format!("invalid SBAS hex block: {e}")))?;
        bytes.push(byte);
    }
    let form = match bytes.len() {
        32 => SbasWireForm::Framed250,
        29 => SbasWireForm::Body226,
        _ => return Err(Error::Parse("invalid SBAS hex block length".to_string())),
    };
    Ok((form, bytes))
}

fn looks_hex(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c.is_whitespace())
}

fn parse_u16(value: &str) -> Option<u16> {
    value.trim().parse().ok()
}

fn parse_u32(value: &str) -> Option<u32> {
    value.trim().parse().ok()
}

fn parse_i64(value: &str) -> Option<i64> {
    value.trim().parse().ok()
}

fn parse_f64(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sbas::message::{SbasBlock, SbasMessage, SbasPrnMask, SpareBits};

    fn sample_body_hex() -> String {
        let mut mask = [false; 210];
        mask[0] = true;
        let bytes = SbasBlock {
            form: SbasWireForm::Body226,
            message: SbasMessage::PrnMask(SbasPrnMask {
                preamble: 0x53,
                iodp: 1,
                mask,
                reserved: SpareBits::new(),
            }),
        }
        .encode();
        bytes.iter().map(|b| format!("{b:02X}")).collect()
    }

    #[test]
    fn rtklib_lines_parse_body_blocks_and_skip_malformed_lines() {
        let hex = sample_body_hex();
        let text = format!("bad line\n2360 259200 120 1 : {hex}\n");
        let parsed = parse_rtklib_lines(&text).expect("parse RTKLIB lines");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].satellite_id.to_string(), "S20");
        assert_eq!(parsed[0].form, SbasWireForm::Body226);
        assert_eq!(parsed[0].bytes.len(), 29);
    }

    #[test]
    fn ems_lines_parse_calendar_epochs() {
        let hex = sample_body_hex();
        let text = format!("120,26,7,1,0,0,1,1,{hex}\nnot,enough\n");
        let parsed = parse_ems_lines(&text).expect("parse EMS lines");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].satellite_id.to_string(), "S20");
        assert_eq!(parsed[0].form, SbasWireForm::Body226);
    }
}

use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq)]
pub enum Field<T> {
    Parsed(T),
    Empty,
    Raw(String),
}

impl<T> Field<T> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Field::Parsed(value) => Some(value),
            Field::Empty | Field::Raw(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StrAuth {
    None,
    Basic,
    Digest,
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sourcetable {
    pub records: Vec<SourcetableRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SourcetableRecord {
    Str(StrRecord),
    Cas(CasRecord),
    Net(NetRecord),
    Other(OtherRecord),
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrRecord {
    pub mountpoint: String,
    pub identifier: String,
    pub format: String,
    pub format_details: String,
    pub carrier: Field<u8>,
    pub nav_system: String,
    pub network: String,
    pub country: String,
    pub lat_deg: Field<f64>,
    pub lon_deg: Field<f64>,
    pub nmea_required: Field<bool>,
    pub network_solution: Field<bool>,
    pub generator: String,
    pub compression: String,
    pub authentication: StrAuth,
    pub fee: Field<bool>,
    pub bitrate: Field<u32>,
    pub misc: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CasRecord {
    pub host: String,
    pub port: Field<u16>,
    pub identifier: String,
    pub operator: String,
    pub nmea_required: Field<bool>,
    pub country: String,
    pub lat_deg: Field<f64>,
    pub lon_deg: Field<f64>,
    pub fallback_host: String,
    pub fallback_port: Field<u16>,
    pub misc: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetRecord {
    pub identifier: String,
    pub operator: String,
    pub authentication: StrAuth,
    pub fee: Field<bool>,
    pub web_net: String,
    pub web_str: String,
    pub web_reg: String,
    pub misc: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtherRecord {
    pub type_tag: String,
    pub fields: Vec<String>,
}

pub fn parse_sourcetable(text: &str) -> Result<Sourcetable> {
    let mut records = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(';').collect();
        let tag = fields[0].trim();
        if tag.eq_ignore_ascii_case("ENDSOURCETABLE") {
            break;
        }
        let record = if tag.eq_ignore_ascii_case("STR") {
            SourcetableRecord::Str(parse_str(&fields))
        } else if tag.eq_ignore_ascii_case("CAS") {
            SourcetableRecord::Cas(parse_cas(&fields))
        } else if tag.eq_ignore_ascii_case("NET") {
            SourcetableRecord::Net(parse_net(&fields))
        } else {
            SourcetableRecord::Other(OtherRecord {
                type_tag: fields[0].to_string(),
                fields: fields.iter().skip(1).map(|s| (*s).to_string()).collect(),
            })
        };
        records.push(record);
    }
    Ok(Sourcetable { records })
}

impl Sourcetable {
    pub fn to_text(&self) -> Result<String> {
        let mut out = String::new();
        for record in &self.records {
            out.push_str(&record.to_line()?);
            out.push_str("\r\n");
        }
        out.push_str("ENDSOURCETABLE\r\n");
        Ok(out)
    }

    pub fn streams(&self) -> impl Iterator<Item = &StrRecord> {
        self.records.iter().filter_map(|record| match record {
            SourcetableRecord::Str(record) => Some(record),
            _ => None,
        })
    }
}

impl SourcetableRecord {
    fn to_line(&self) -> Result<String> {
        match self {
            SourcetableRecord::Str(record) => record.to_line(),
            SourcetableRecord::Cas(record) => record.to_line(),
            SourcetableRecord::Net(record) => record.to_line(),
            SourcetableRecord::Other(record) => {
                let mut fields = vec![ordinary_text(&record.type_tag, "other type tag")?];
                for field in &record.fields {
                    fields.push(ordinary_text(field, "other field")?);
                }
                Ok(fields.join(";"))
            }
        }
    }
}

impl StrRecord {
    fn to_line(&self) -> Result<String> {
        Ok([
            "STR".to_string(),
            ordinary_text(&self.mountpoint, "STR mountpoint")?,
            ordinary_text(&self.identifier, "STR identifier")?,
            ordinary_text(&self.format, "STR format")?,
            ordinary_text(&self.format_details, "STR format details")?,
            field_to_string(&self.carrier, "STR carrier")?,
            ordinary_text(&self.nav_system, "STR nav system")?,
            ordinary_text(&self.network, "STR network")?,
            ordinary_text(&self.country, "STR country")?,
            field_to_string(&self.lat_deg, "STR latitude")?,
            field_to_string(&self.lon_deg, "STR longitude")?,
            bool01_to_string(&self.nmea_required, "STR NMEA required")?,
            bool01_to_string(&self.network_solution, "STR network solution")?,
            ordinary_text(&self.generator, "STR generator")?,
            ordinary_text(&self.compression, "STR compression")?,
            auth_to_string(&self.authentication, "STR authentication")?,
            boolyn_to_string(&self.fee, "STR fee")?,
            field_to_string(&self.bitrate, "STR bitrate")?,
            tail_text(&self.misc, "STR misc")?,
        ]
        .join(";"))
    }
}

impl CasRecord {
    fn to_line(&self) -> Result<String> {
        Ok([
            "CAS".to_string(),
            ordinary_text(&self.host, "CAS host")?,
            field_to_string(&self.port, "CAS port")?,
            ordinary_text(&self.identifier, "CAS identifier")?,
            ordinary_text(&self.operator, "CAS operator")?,
            bool01_to_string(&self.nmea_required, "CAS NMEA required")?,
            ordinary_text(&self.country, "CAS country")?,
            field_to_string(&self.lat_deg, "CAS latitude")?,
            field_to_string(&self.lon_deg, "CAS longitude")?,
            ordinary_text(&self.fallback_host, "CAS fallback host")?,
            field_to_string(&self.fallback_port, "CAS fallback port")?,
            tail_text(&self.misc, "CAS misc")?,
        ]
        .join(";"))
    }
}

impl NetRecord {
    fn to_line(&self) -> Result<String> {
        Ok([
            "NET".to_string(),
            ordinary_text(&self.identifier, "NET identifier")?,
            ordinary_text(&self.operator, "NET operator")?,
            auth_to_string(&self.authentication, "NET authentication")?,
            boolyn_to_string(&self.fee, "NET fee")?,
            ordinary_text(&self.web_net, "NET web net")?,
            ordinary_text(&self.web_str, "NET web str")?,
            ordinary_text(&self.web_reg, "NET web reg")?,
            tail_text(&self.misc, "NET misc")?,
        ]
        .join(";"))
    }
}

fn parse_str(fields: &[&str]) -> StrRecord {
    StrRecord {
        mountpoint: get(fields, 1).to_string(),
        identifier: get(fields, 2).to_string(),
        format: get(fields, 3).to_string(),
        format_details: get(fields, 4).to_string(),
        carrier: parse_field(get(fields, 5)),
        nav_system: get(fields, 6).to_string(),
        network: get(fields, 7).to_string(),
        country: get(fields, 8).to_string(),
        lat_deg: parse_finite_f64_field(get(fields, 9)),
        lon_deg: parse_finite_f64_field(get(fields, 10)),
        nmea_required: parse_bool01(get(fields, 11)),
        network_solution: parse_bool01(get(fields, 12)),
        generator: get(fields, 13).to_string(),
        compression: get(fields, 14).to_string(),
        authentication: parse_auth(get(fields, 15)),
        fee: parse_boolyn(get(fields, 16)),
        bitrate: parse_field(get(fields, 17)),
        misc: join_tail(fields, 18),
    }
}

fn parse_cas(fields: &[&str]) -> CasRecord {
    CasRecord {
        host: get(fields, 1).to_string(),
        port: parse_field(get(fields, 2)),
        identifier: get(fields, 3).to_string(),
        operator: get(fields, 4).to_string(),
        nmea_required: parse_bool01(get(fields, 5)),
        country: get(fields, 6).to_string(),
        lat_deg: parse_finite_f64_field(get(fields, 7)),
        lon_deg: parse_finite_f64_field(get(fields, 8)),
        fallback_host: get(fields, 9).to_string(),
        fallback_port: parse_field(get(fields, 10)),
        misc: join_tail(fields, 11),
    }
}

fn parse_net(fields: &[&str]) -> NetRecord {
    NetRecord {
        identifier: get(fields, 1).to_string(),
        operator: get(fields, 2).to_string(),
        authentication: parse_auth(get(fields, 3)),
        fee: parse_boolyn(get(fields, 4)),
        web_net: get(fields, 5).to_string(),
        web_str: get(fields, 6).to_string(),
        web_reg: get(fields, 7).to_string(),
        misc: join_tail(fields, 8),
    }
}

fn get<'a>(fields: &'a [&str], index: usize) -> &'a str {
    fields.get(index).copied().unwrap_or("")
}

fn join_tail(fields: &[&str], index: usize) -> String {
    if index >= fields.len() {
        String::new()
    } else {
        fields[index..].join(";")
    }
}

fn parse_field<T>(value: &str) -> Field<T>
where
    T: core::str::FromStr,
{
    if value.is_empty() {
        Field::Empty
    } else {
        value
            .parse()
            .map(Field::Parsed)
            .unwrap_or_else(|_| Field::Raw(value.to_string()))
    }
}

fn parse_finite_f64_field(value: &str) -> Field<f64> {
    if value.is_empty() {
        Field::Empty
    } else {
        match value.parse::<f64>() {
            Ok(parsed) if parsed.is_finite() => Field::Parsed(parsed),
            _ => Field::Raw(value.to_string()),
        }
    }
}

fn parse_bool01(value: &str) -> Field<bool> {
    match value {
        "" => Field::Empty,
        "0" => Field::Parsed(false),
        "1" => Field::Parsed(true),
        _ => Field::Raw(value.to_string()),
    }
}

fn parse_boolyn(value: &str) -> Field<bool> {
    match value {
        "" => Field::Empty,
        "N" => Field::Parsed(false),
        "Y" => Field::Parsed(true),
        _ => Field::Raw(value.to_string()),
    }
}

fn parse_auth(value: &str) -> StrAuth {
    match value {
        "N" => StrAuth::None,
        "B" => StrAuth::Basic,
        "D" => StrAuth::Digest,
        other => StrAuth::Other(other.to_string()),
    }
}

fn field_to_string<T: ToString>(field: &Field<T>, name: &str) -> Result<String> {
    match field {
        Field::Parsed(value) => ordinary_text(&value.to_string(), name),
        Field::Empty => Ok(String::new()),
        Field::Raw(value) => ordinary_text(value, name),
    }
}

fn bool01_to_string(field: &Field<bool>, name: &str) -> Result<String> {
    match field {
        Field::Parsed(false) => Ok("0".into()),
        Field::Parsed(true) => Ok("1".into()),
        Field::Empty => Ok(String::new()),
        Field::Raw(value) => ordinary_text(value, name),
    }
}

fn boolyn_to_string(field: &Field<bool>, name: &str) -> Result<String> {
    match field {
        Field::Parsed(false) => Ok("N".into()),
        Field::Parsed(true) => Ok("Y".into()),
        Field::Empty => Ok(String::new()),
        Field::Raw(value) => ordinary_text(value, name),
    }
}

fn auth_to_string(auth: &StrAuth, name: &str) -> Result<String> {
    match auth {
        StrAuth::None => Ok("N".into()),
        StrAuth::Basic => Ok("B".into()),
        StrAuth::Digest => Ok("D".into()),
        StrAuth::Other(value) => ordinary_text(value, name),
    }
}

fn ordinary_text(value: &str, name: &str) -> Result<String> {
    checked_text(value, name, false)
}

fn tail_text(value: &str, name: &str) -> Result<String> {
    checked_text(value, name, true)
}

fn checked_text(value: &str, name: &str, allow_semicolon: bool) -> Result<String> {
    if value.contains('\r') || value.contains('\n') {
        return Err(Error::InvalidInput(format!(
            "sourcetable {name} contains a line break"
        )));
    }
    if !allow_semicolon && value.contains(';') {
        return Err(Error::InvalidInput(format!(
            "sourcetable {name} contains a semicolon"
        )));
    }
    Ok(value.to_string())
}

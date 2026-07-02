use crate::Result;

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
        let field_storage = split_fields(line);
        let fields: Vec<&str> = field_storage.iter().map(String::as_str).collect();
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
                type_tag: unescape_text(fields[0]),
                fields: fields.iter().skip(1).map(|s| unescape_text(s)).collect(),
            })
        };
        records.push(record);
    }
    Ok(Sourcetable { records })
}

impl Sourcetable {
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for record in &self.records {
            out.push_str(&record.to_line());
            out.push_str("\r\n");
        }
        out.push_str("ENDSOURCETABLE\r\n");
        out
    }

    pub fn streams(&self) -> impl Iterator<Item = &StrRecord> {
        self.records.iter().filter_map(|record| match record {
            SourcetableRecord::Str(record) => Some(record),
            _ => None,
        })
    }
}

impl SourcetableRecord {
    fn to_line(&self) -> String {
        match self {
            SourcetableRecord::Str(record) => record.to_line(),
            SourcetableRecord::Cas(record) => record.to_line(),
            SourcetableRecord::Net(record) => record.to_line(),
            SourcetableRecord::Other(record) => {
                let mut fields = vec![escape_text(&record.type_tag)];
                fields.extend(record.fields.iter().map(|field| escape_text(field)));
                fields.join(";")
            }
        }
    }
}

impl StrRecord {
    fn to_line(&self) -> String {
        [
            "STR".to_string(),
            escape_text(&self.mountpoint),
            escape_text(&self.identifier),
            escape_text(&self.format),
            escape_text(&self.format_details),
            field_to_string(&self.carrier),
            escape_text(&self.nav_system),
            escape_text(&self.network),
            escape_text(&self.country),
            field_to_string(&self.lat_deg),
            field_to_string(&self.lon_deg),
            bool01_to_string(&self.nmea_required),
            bool01_to_string(&self.network_solution),
            escape_text(&self.generator),
            escape_text(&self.compression),
            auth_to_string(&self.authentication),
            boolyn_to_string(&self.fee),
            field_to_string(&self.bitrate),
            escape_text(&self.misc),
        ]
        .join(";")
    }
}

impl CasRecord {
    fn to_line(&self) -> String {
        [
            "CAS".to_string(),
            escape_text(&self.host),
            field_to_string(&self.port),
            escape_text(&self.identifier),
            escape_text(&self.operator),
            bool01_to_string(&self.nmea_required),
            escape_text(&self.country),
            field_to_string(&self.lat_deg),
            field_to_string(&self.lon_deg),
            escape_text(&self.fallback_host),
            field_to_string(&self.fallback_port),
            escape_text(&self.misc),
        ]
        .join(";")
    }
}

impl NetRecord {
    fn to_line(&self) -> String {
        [
            "NET".to_string(),
            escape_text(&self.identifier),
            escape_text(&self.operator),
            auth_to_string(&self.authentication),
            boolyn_to_string(&self.fee),
            escape_text(&self.web_net),
            escape_text(&self.web_str),
            escape_text(&self.web_reg),
            escape_text(&self.misc),
        ]
        .join(";")
    }
}

fn parse_str(fields: &[&str]) -> StrRecord {
    StrRecord {
        mountpoint: unescape_text(get(fields, 1)),
        identifier: unescape_text(get(fields, 2)),
        format: unescape_text(get(fields, 3)),
        format_details: unescape_text(get(fields, 4)),
        carrier: parse_field(get(fields, 5)),
        nav_system: unescape_text(get(fields, 6)),
        network: unescape_text(get(fields, 7)),
        country: unescape_text(get(fields, 8)),
        lat_deg: parse_finite_f64_field(get(fields, 9)),
        lon_deg: parse_finite_f64_field(get(fields, 10)),
        nmea_required: parse_bool01(get(fields, 11)),
        network_solution: parse_bool01(get(fields, 12)),
        generator: unescape_text(get(fields, 13)),
        compression: unescape_text(get(fields, 14)),
        authentication: parse_auth(get(fields, 15)),
        fee: parse_boolyn(get(fields, 16)),
        bitrate: parse_field(get(fields, 17)),
        misc: join_tail(fields, 18),
    }
}

fn parse_cas(fields: &[&str]) -> CasRecord {
    CasRecord {
        host: unescape_text(get(fields, 1)),
        port: parse_field(get(fields, 2)),
        identifier: unescape_text(get(fields, 3)),
        operator: unescape_text(get(fields, 4)),
        nmea_required: parse_bool01(get(fields, 5)),
        country: unescape_text(get(fields, 6)),
        lat_deg: parse_finite_f64_field(get(fields, 7)),
        lon_deg: parse_finite_f64_field(get(fields, 8)),
        fallback_host: unescape_text(get(fields, 9)),
        fallback_port: parse_field(get(fields, 10)),
        misc: join_tail(fields, 11),
    }
}

fn parse_net(fields: &[&str]) -> NetRecord {
    NetRecord {
        identifier: unescape_text(get(fields, 1)),
        operator: unescape_text(get(fields, 2)),
        authentication: parse_auth(get(fields, 3)),
        fee: parse_boolyn(get(fields, 4)),
        web_net: unescape_text(get(fields, 5)),
        web_str: unescape_text(get(fields, 6)),
        web_reg: unescape_text(get(fields, 7)),
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
        unescape_text(&fields[index..].join(";"))
    }
}

fn split_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            field.push(ch);
            if let Some(next) = chars.next() {
                field.push(next);
            }
        } else if ch == ';' {
            fields.push(field);
            field = String::new();
        } else {
            field.push(ch);
        }
    }
    fields.push(field);
    fields
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

fn field_to_string<T: ToString>(field: &Field<T>) -> String {
    match field {
        Field::Parsed(value) => value.to_string(),
        Field::Empty => String::new(),
        Field::Raw(value) => escape_text(value),
    }
}

fn bool01_to_string(field: &Field<bool>) -> String {
    match field {
        Field::Parsed(false) => "0".into(),
        Field::Parsed(true) => "1".into(),
        Field::Empty => String::new(),
        Field::Raw(value) => value.clone(),
    }
}

fn boolyn_to_string(field: &Field<bool>) -> String {
    match field {
        Field::Parsed(false) => "N".into(),
        Field::Parsed(true) => "Y".into(),
        Field::Empty => String::new(),
        Field::Raw(value) => value.clone(),
    }
}

fn auth_to_string(auth: &StrAuth) -> String {
    match auth {
        StrAuth::None => "N".into(),
        StrAuth::Basic => "B".into(),
        StrAuth::Digest => "D".into(),
        StrAuth::Other(value) => escape_text(value),
    }
}

fn escape_text(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

fn unescape_text(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some(';') => out.push(';'),
                Some('r') => out.push('\r'),
                Some('n') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

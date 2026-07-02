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
        let fields: Vec<&str> = line.split(';').collect();
        let tag = fields[0].trim();
        if tag.eq_ignore_ascii_case("ENDSOURCETABLE") {
            break;
        }
        let record = match tag {
            "STR" => SourcetableRecord::Str(parse_str(&fields)),
            "CAS" => SourcetableRecord::Cas(parse_cas(&fields)),
            "NET" => SourcetableRecord::Net(parse_net(&fields)),
            _ => SourcetableRecord::Other(OtherRecord {
                type_tag: tag.to_string(),
                fields: fields.iter().skip(1).map(|s| (*s).to_string()).collect(),
            }),
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
                let mut fields = vec![record.type_tag.clone()];
                fields.extend(record.fields.clone());
                fields.join(";")
            }
        }
    }
}

impl StrRecord {
    fn to_line(&self) -> String {
        [
            "STR".to_string(),
            self.mountpoint.clone(),
            self.identifier.clone(),
            self.format.clone(),
            self.format_details.clone(),
            field_to_string(&self.carrier),
            self.nav_system.clone(),
            self.network.clone(),
            self.country.clone(),
            field_to_string(&self.lat_deg),
            field_to_string(&self.lon_deg),
            bool01_to_string(&self.nmea_required),
            bool01_to_string(&self.network_solution),
            self.generator.clone(),
            self.compression.clone(),
            auth_to_string(&self.authentication),
            boolyn_to_string(&self.fee),
            field_to_string(&self.bitrate),
            self.misc.clone(),
        ]
        .join(";")
    }
}

impl CasRecord {
    fn to_line(&self) -> String {
        [
            "CAS".to_string(),
            self.host.clone(),
            field_to_string(&self.port),
            self.identifier.clone(),
            self.operator.clone(),
            bool01_to_string(&self.nmea_required),
            self.country.clone(),
            field_to_string(&self.lat_deg),
            field_to_string(&self.lon_deg),
            self.fallback_host.clone(),
            field_to_string(&self.fallback_port),
            self.misc.clone(),
        ]
        .join(";")
    }
}

impl NetRecord {
    fn to_line(&self) -> String {
        [
            "NET".to_string(),
            self.identifier.clone(),
            self.operator.clone(),
            auth_to_string(&self.authentication),
            boolyn_to_string(&self.fee),
            self.web_net.clone(),
            self.web_str.clone(),
            self.web_reg.clone(),
            self.misc.clone(),
        ]
        .join(";")
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
        lat_deg: parse_field(get(fields, 9)),
        lon_deg: parse_field(get(fields, 10)),
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
        lat_deg: parse_field(get(fields, 7)),
        lon_deg: parse_field(get(fields, 8)),
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
        Field::Raw(value) => value.clone(),
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
        StrAuth::Other(value) => value.clone(),
    }
}

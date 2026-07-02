use crate::ntrip::chunk::ChunkedDecoder;
use crate::ntrip::gga::{format_gga, GgaPosition};
use crate::ntrip::request::{NtripConfig, NtripVersion};
use crate::ntrip::response::{classify_http_response, HttpClassification, NtripRejection};
use crate::ntrip::sourcetable::{parse_sourcetable, Sourcetable};
use crate::Result;

const MAX_LINE: usize = 8 * 1024;
const MAX_HEADER_BLOCK: usize = 64 * 1024;
const MAX_SOURCETABLE: usize = 4 * 1024 * 1024;
const PREFIX_LIMIT: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NtripState {
    Idle,
    AwaitingStatus,
    AwaitingHeaders,
    Streaming,
    Sourcetable,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NtripHandshake {
    pub version: NtripVersion,
    pub chunked: bool,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NtripEvent {
    Connected(NtripHandshake),
    Payload(Vec<u8>),
    Sourcetable(Sourcetable),
    Rejected(NtripRejection),
    StreamCorrupted { detail: String },
    StreamEnded,
}

#[derive(Clone, Debug)]
pub struct NtripClientMachine {
    config: NtripConfig,
    state: NtripState,
    carry: Vec<u8>,
    headers: Vec<(String, String)>,
    header_bytes: usize,
    status_version: Option<NtripVersion>,
    chunked: bool,
    chunk_decoder: ChunkedDecoder,
    sourcetable_text: String,
    sourcetable_records_started: bool,
    last_gga_s: Option<f64>,
}

impl NtripClientMachine {
    pub fn new(config: NtripConfig) -> Self {
        Self {
            config,
            state: NtripState::Idle,
            carry: Vec::new(),
            headers: Vec::new(),
            header_bytes: 0,
            status_version: None,
            chunked: false,
            chunk_decoder: ChunkedDecoder::new(),
            sourcetable_text: String::new(),
            sourcetable_records_started: false,
            last_gga_s: None,
        }
    }

    pub fn connection_request(&mut self) -> Result<Vec<u8>> {
        let bytes = self.config.request_bytes()?;
        self.state = NtripState::AwaitingStatus;
        Ok(bytes)
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<NtripEvent> {
        let mut events = Vec::new();
        if matches!(self.state, NtripState::Closed) {
            return events;
        }
        self.carry.extend_from_slice(bytes);

        loop {
            match self.state {
                NtripState::Idle => {
                    self.state = NtripState::AwaitingStatus;
                }
                NtripState::AwaitingStatus => {
                    if !self.parse_status(&mut events) {
                        break;
                    }
                }
                NtripState::AwaitingHeaders => {
                    if !self.parse_headers(&mut events) {
                        break;
                    }
                }
                NtripState::Streaming => {
                    self.drain_payload(&mut events);
                    break;
                }
                NtripState::Sourcetable => {
                    if !self.drain_sourcetable(&mut events) {
                        break;
                    }
                }
                NtripState::Closed => break,
            }
        }

        events
    }

    pub fn gga_message(
        &mut self,
        now_s: f64,
        position: &GgaPosition,
        utc_seconds_of_day: f64,
    ) -> Option<Vec<u8>> {
        if self.state != NtripState::Streaming {
            return None;
        }
        let interval = self.config.gga_interval_s?;
        if !now_s.is_finite() {
            return None;
        }
        let due = match self.last_gga_s {
            None => true,
            Some(last) if now_s >= last => now_s - last >= interval,
            Some(_) => false,
        };
        if !due {
            return None;
        }
        let bytes = format_gga(position, utc_seconds_of_day).ok()?;
        self.last_gga_s = Some(now_s);
        Some(bytes)
    }

    pub fn state(&self) -> NtripState {
        self.state
    }

    pub fn reset(&mut self) {
        let config = self.config.clone();
        *self = Self::new(config);
    }

    fn parse_status(&mut self, events: &mut Vec<NtripEvent>) -> bool {
        let Some(line) = self.take_line_or_reject(events) else {
            return false;
        };
        let text = String::from_utf8_lossy(&line).trim().to_string();

        if let Some(rest) = text.strip_prefix("ERROR - ") {
            let reason = rest.to_string();
            let rejection = if reason.to_ascii_lowercase().contains("password") {
                NtripRejection::Unauthorized
            } else {
                NtripRejection::CasterError { reason }
            };
            self.reject(events, rejection);
            return true;
        }

        if let Some((status, reason)) = parse_prefixed_status(&text, "ICY") {
            if status == 200 {
                self.state = NtripState::Streaming;
                self.status_version = Some(NtripVersion::Rev1);
                self.chunked = false;
                self.consume_optional_blank_line();
                events.push(NtripEvent::Connected(NtripHandshake {
                    version: NtripVersion::Rev1,
                    chunked: false,
                    headers: Vec::new(),
                }));
                return true;
            }
            self.reject(
                events,
                NtripRejection::HttpError {
                    status,
                    reason: reason.to_string(),
                },
            );
            return true;
        }

        if let Some((status, reason)) = parse_prefixed_status(&text, "SOURCETABLE") {
            if status == 200 {
                self.state = NtripState::Sourcetable;
                self.status_version = Some(NtripVersion::Rev1);
                return true;
            }
            self.reject(
                events,
                NtripRejection::HttpError {
                    status,
                    reason: reason.to_string(),
                },
            );
            return true;
        }

        if let Some((version, status, reason)) = parse_http_status(&text) {
            self.status_version = Some(version);
            self.headers.clear();
            self.header_bytes = 0;
            self.state = NtripState::AwaitingHeaders;
            self.headers.push((":status".into(), status.to_string()));
            self.headers.push((":reason".into(), reason.to_string()));
            return true;
        }

        self.reject(
            events,
            NtripRejection::MalformedHandshake {
                prefix: self.prefix_with_line(&line),
            },
        );
        true
    }

    fn parse_headers(&mut self, events: &mut Vec<NtripEvent>) -> bool {
        let Some(line) = self.take_line_or_reject(events) else {
            return false;
        };
        if line.is_empty() {
            let status = self
                .headers
                .iter()
                .find(|(name, _)| name == ":status")
                .and_then(|(_, value)| value.parse::<u16>().ok())
                .unwrap_or(0);
            let reason = self
                .headers
                .iter()
                .find(|(name, _)| name == ":reason")
                .map(|(_, value)| value.as_str())
                .unwrap_or("");
            let real_headers: Vec<(String, String)> = self
                .headers
                .iter()
                .filter(|(name, _)| !name.starts_with(':'))
                .cloned()
                .collect();
            match classify_http_response(status, reason, &real_headers) {
                HttpClassification::Stream { chunked } => {
                    self.chunked = chunked;
                    self.state = NtripState::Streaming;
                    events.push(NtripEvent::Connected(NtripHandshake {
                        version: self.status_version.unwrap_or(NtripVersion::Rev2),
                        chunked,
                        headers: real_headers,
                    }));
                    true
                }
                HttpClassification::Sourcetable => {
                    self.state = NtripState::Sourcetable;
                    self.sourcetable_records_started = true;
                    true
                }
                HttpClassification::Rejection(rejection) => {
                    self.reject(events, rejection);
                    true
                }
            }
        } else {
            self.header_bytes += line.len();
            if self.header_bytes > MAX_HEADER_BLOCK {
                self.reject_current_prefix(events);
                return true;
            }
            if let Some((name, value)) = split_header(&line) {
                self.headers.push((name, value));
            }
            true
        }
    }

    fn drain_payload(&mut self, events: &mut Vec<NtripEvent>) {
        if self.carry.is_empty() {
            return;
        }
        let bytes: Vec<u8> = self.carry.drain(..).collect();
        if self.chunked {
            match self.chunk_decoder.push(&bytes) {
                Ok(payload) => {
                    if !payload.is_empty() {
                        events.push(NtripEvent::Payload(payload));
                    }
                    if self.chunk_decoder.finished() {
                        self.state = NtripState::Closed;
                        events.push(NtripEvent::StreamEnded);
                    }
                }
                Err(err) => {
                    self.state = NtripState::Closed;
                    events.push(NtripEvent::StreamCorrupted {
                        detail: err.to_string(),
                    });
                }
            }
        } else {
            events.push(NtripEvent::Payload(bytes));
        }
    }

    fn drain_sourcetable(&mut self, events: &mut Vec<NtripEvent>) -> bool {
        while let Some(line) = self.take_line_or_reject(events) {
            let text = String::from_utf8_lossy(&line).to_string();
            let first = text.split(';').next().unwrap_or("").trim();

            if !self.sourcetable_records_started {
                if text.is_empty() {
                    continue;
                }
                if matches!(first, "STR" | "CAS" | "NET" | "ENDSOURCETABLE") {
                    self.sourcetable_records_started = true;
                } else if text.contains(':') {
                    continue;
                } else {
                    self.sourcetable_records_started = true;
                }
            }

            self.sourcetable_text.push_str(&text);
            self.sourcetable_text.push_str("\r\n");
            if self.sourcetable_text.len() > MAX_SOURCETABLE {
                self.reject_current_prefix(events);
                return true;
            }
            if first == "ENDSOURCETABLE" {
                match parse_sourcetable(&self.sourcetable_text) {
                    Ok(table) => events.push(NtripEvent::Sourcetable(table)),
                    Err(err) => events.push(NtripEvent::StreamCorrupted {
                        detail: err.to_string(),
                    }),
                }
                self.state = NtripState::Closed;
                return true;
            }
        }
        false
    }

    fn take_line_or_reject(&mut self, events: &mut Vec<NtripEvent>) -> Option<Vec<u8>> {
        if let Some(pos) = self.carry.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.carry.drain(..=pos).collect();
            if line.ends_with(b"\n") {
                line.pop();
            }
            if line.ends_with(b"\r") {
                line.pop();
            }
            Some(line)
        } else if self.carry.len() > MAX_LINE {
            self.reject_current_prefix(events);
            None
        } else {
            None
        }
    }

    fn consume_optional_blank_line(&mut self) {
        if self.carry.starts_with(b"\r\n") {
            self.carry.drain(..2);
        } else if self.carry.starts_with(b"\n") {
            self.carry.drain(..1);
        }
    }

    fn reject_current_prefix(&mut self, events: &mut Vec<NtripEvent>) {
        let prefix = self.carry.iter().copied().take(PREFIX_LIMIT).collect();
        self.reject(events, NtripRejection::MalformedHandshake { prefix });
    }

    fn reject(&mut self, events: &mut Vec<NtripEvent>, rejection: NtripRejection) {
        self.state = NtripState::Closed;
        events.push(NtripEvent::Rejected(rejection));
    }

    fn prefix_with_line(&self, line: &[u8]) -> Vec<u8> {
        let mut prefix = line.to_vec();
        prefix.extend_from_slice(&self.carry);
        prefix.truncate(PREFIX_LIMIT);
        prefix
    }
}

fn parse_prefixed_status<'a>(text: &'a str, prefix: &str) -> Option<(u16, &'a str)> {
    let rest = text.strip_prefix(prefix)?.trim_start();
    let mut parts = rest.splitn(2, char::is_whitespace);
    let status = parts.next()?.parse().ok()?;
    let reason = parts.next().unwrap_or("").trim();
    Some((status, reason))
}

fn parse_http_status(text: &str) -> Option<(NtripVersion, u16, &str)> {
    let rest = if let Some(rest) = text.strip_prefix("HTTP/1.1 ") {
        (NtripVersion::Rev2, rest)
    } else if let Some(rest) = text.strip_prefix("HTTP/1.0 ") {
        (NtripVersion::Rev1, rest)
    } else {
        return None;
    };
    let mut parts = rest.1.splitn(2, char::is_whitespace);
    let status = parts.next()?.parse().ok()?;
    let reason = parts.next().unwrap_or("").trim();
    Some((rest.0, status, reason))
}

fn split_header(line: &[u8]) -> Option<(String, String)> {
    let pos = line.iter().position(|&b| b == b':')?;
    let name = String::from_utf8_lossy(&line[..pos]).trim().to_string();
    let value = String::from_utf8_lossy(&line[pos + 1..]).trim().to_string();
    Some((name, value))
}

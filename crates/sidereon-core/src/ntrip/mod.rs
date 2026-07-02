//! NTRIP client request, response, and stream handling.
//!
//! This module is deliberately sans-I/O. Transports own sockets, TLS, DNS,
//! clocks, retries, and storage. The core machine accepts received bytes and
//! emits protocol events, including opaque payload bytes that can be passed to
//! [`crate::rtcm::SsrStreamAssembler`] or another downstream decoder.

mod chunk;
mod gga;
mod machine;
mod request;
mod response;
mod sourcetable;

#[cfg(test)]
mod tests;

pub use chunk::ChunkedDecoder;
pub use gga::{format_gga, GgaPosition};
pub use machine::{NtripClientMachine, NtripEvent, NtripHandshake, NtripState};
pub use request::{NtripConfig, NtripCredentials, NtripVersion};
pub use response::{classify_http_response, HttpClassification, NtripRejection};
pub use sourcetable::{
    parse_sourcetable, CasRecord, Field, NetRecord, OtherRecord, Sourcetable, SourcetableRecord,
    StrAuth, StrRecord,
};

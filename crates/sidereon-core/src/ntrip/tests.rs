use super::*;
use crate::rtcm::{self, Message};

fn config(version: NtripVersion) -> NtripConfig {
    NtripConfig {
        host: "caster.example.test".into(),
        port: 2101,
        mountpoint: "MOUNT".into(),
        version,
        credentials: None,
        user_agent_product: "sidereon-test/0".into(),
        gga_interval_s: None,
    }
}

#[test]
fn rev1_request_bytes_are_deterministic() {
    let request = config(NtripVersion::Rev1).request_bytes().unwrap();
    assert_eq!(
        request,
        b"GET /MOUNT HTTP/1.0\r\nUser-Agent: NTRIP sidereon-test/0\r\n\r\n"
    );
}

#[test]
fn rev2_request_bytes_and_headers_use_same_values() {
    let mut cfg = config(NtripVersion::Rev2);
    cfg.credentials = Some(NtripCredentials {
        username: "user".into(),
        password: "pass".into(),
    });

    let request = String::from_utf8(cfg.request_bytes().unwrap()).unwrap();
    assert_eq!(
        request,
        "GET /MOUNT HTTP/1.1\r\nHost: caster.example.test:2101\r\nNtrip-Version: Ntrip/2.0\r\nUser-Agent: NTRIP sidereon-test/0\r\nAuthorization: Basic dXNlcjpwYXNz\r\nConnection: close\r\n\r\n"
    );

    let (path, headers) = cfg.request_headers().unwrap();
    assert_eq!(path, "/MOUNT");
    assert_eq!(
        headers,
        vec![
            ("Host".into(), "caster.example.test:2101".into()),
            ("Ntrip-Version".into(), "Ntrip/2.0".into()),
            ("User-Agent".into(), "NTRIP sidereon-test/0".into()),
            ("Authorization".into(), "Basic dXNlcjpwYXNz".into()),
            ("Connection".into(), "close".into()),
        ]
    );
}

#[test]
fn request_validation_rejects_malformed_values() {
    let mut cfg = config(NtripVersion::Rev2);
    cfg.mountpoint = "BAD/MOUNT".into();
    assert!(cfg.request_bytes().is_err());

    let mut cfg = config(NtripVersion::Rev2);
    cfg.user_agent_product = "bad product".into();
    assert!(cfg.request_bytes().is_err());

    let mut cfg = config(NtripVersion::Rev2);
    cfg.credentials = Some(NtripCredentials {
        username: "u:ser".into(),
        password: "pass".into(),
    });
    assert!(cfg.request_bytes().is_err());
}

#[test]
fn rev1_icy_stream_emits_payload_after_optional_blank_line() {
    let mut machine = NtripClientMachine::new(config(NtripVersion::Rev1));
    machine.connection_request().unwrap();
    let events = machine.push(b"ICY 200 OK\r\n\r\nabc");

    assert_eq!(
        events,
        vec![
            NtripEvent::Connected(NtripHandshake {
                version: NtripVersion::Rev1,
                chunked: false,
                headers: vec![],
            }),
            NtripEvent::Payload(b"abc".to_vec()),
        ]
    );
    assert_eq!(machine.state(), NtripState::Streaming);
}

#[test]
fn rev2_http_stream_emits_dechunked_payload_split_across_pushes() {
    let frame = rtcm::encode_frame(&[0xff, 0xf0]).unwrap();
    let mut wire =
        b"HTTP/1.1 200 OK\r\nContent-Type: gnss/data\r\nTransfer-Encoding: chunked\r\n\r\n"
            .to_vec();
    wire.extend_from_slice(format!("{:X}\r\n", frame.len()).as_bytes());
    wire.extend_from_slice(&frame[..3]);

    let mut machine = NtripClientMachine::new(config(NtripVersion::Rev2));
    machine.connection_request().unwrap();
    let first = machine.push(&wire);
    assert_eq!(
        first[0],
        NtripEvent::Connected(NtripHandshake {
            version: NtripVersion::Rev2,
            chunked: true,
            headers: vec![
                ("Content-Type".into(), "gnss/data".into()),
                ("Transfer-Encoding".into(), "chunked".into()),
            ],
        })
    );
    assert_eq!(first[1], NtripEvent::Payload(frame[..3].to_vec()));

    let mut tail = frame[3..].to_vec();
    tail.extend_from_slice(b"\r\n0\r\n\r\n");
    let second = machine.push(&tail);
    assert_eq!(
        second,
        vec![
            NtripEvent::Payload(frame[3..].to_vec()),
            NtripEvent::StreamEnded,
        ]
    );

    let mut assembler = rtcm::SsrStreamAssembler::new();
    let decoded: Vec<_> = first
        .into_iter()
        .chain(second)
        .filter_map(|event| match event {
            NtripEvent::Payload(bytes) => Some(bytes),
            _ => None,
        })
        .flat_map(|bytes| assembler.push(&bytes))
        .collect();
    assert!(matches!(&decoded[0], Ok(Message::Unsupported(msg)) if msg.message_number == 4095));
}

#[test]
fn chunk_decoder_handles_extensions_trailers_and_poisoning() {
    let mut decoder = ChunkedDecoder::new();
    assert_eq!(decoder.push(b"4;ignored=yes\r\nWi").unwrap(), b"Wi");
    assert_eq!(
        decoder
            .push(b"ki\r\n5\r\npedia\r\n0\r\nX: y\r\n\r\n")
            .unwrap(),
        b"kipedia"
    );
    assert!(decoder.finished());

    let mut decoder = ChunkedDecoder::new();
    assert!(decoder.push(b"Z\r\n").is_err());
    assert!(decoder.push(b"1\r\na\r\n").is_err());
}

#[test]
fn rev1_sourcetable_response_parses_records_after_headers() {
    let wire = b"SOURCETABLE 200 OK\r\nServer: caster\r\nContent-Type: gnss/sourcetable\r\n\r\nSTR;MOUNT;ID;RTCM 3;1004(1);2;GPS;NET;USA;40.1;-105.2;1;0;gen;none;B;N;9600;misc;with;semis\r\nCAS;caster.example.test;2101;Caster;Op;0;USA;40.0;-105.0;backup.example.test;2102;cas misc\r\nNET;NET;Op;D;Y;https://net;https://str;https://reg;net misc\r\nENDSOURCETABLE\r\n";
    let mut machine = NtripClientMachine::new(config(NtripVersion::Rev1));
    machine.connection_request().unwrap();
    let events = machine.push(wire);

    let NtripEvent::Sourcetable(table) = &events[0] else {
        panic!("expected sourcetable event");
    };
    assert_eq!(table.records.len(), 3);
    let stream = table.streams().next().unwrap();
    assert_eq!(stream.mountpoint, "MOUNT");
    assert_eq!(stream.nmea_required.value(), Some(&true));
    assert_eq!(stream.authentication, StrAuth::Basic);
    assert_eq!(stream.misc, "misc;with;semis");
}

#[test]
fn bad_auth_status_digest_and_content_type_are_rejections() {
    let mut machine = NtripClientMachine::new(config(NtripVersion::Rev1));
    machine.connection_request().unwrap();
    assert_eq!(
        machine.push(b"ERROR - Bad Password\r\n"),
        vec![NtripEvent::Rejected(NtripRejection::Unauthorized)]
    );

    let mut machine = NtripClientMachine::new(config(NtripVersion::Rev2));
    machine.connection_request().unwrap();
    assert_eq!(
        machine.push(b"HTTP/1.1 404 Not Found\r\n\r\n"),
        vec![NtripEvent::Rejected(NtripRejection::MountpointNotFound)]
    );

    let mut machine = NtripClientMachine::new(config(NtripVersion::Rev2));
    machine.connection_request().unwrap();
    assert_eq!(
        machine.push(
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"caster\"\r\n\r\n"
        ),
        vec![NtripEvent::Rejected(NtripRejection::DigestRequired)]
    );

    let mut machine = NtripClientMachine::new(config(NtripVersion::Rev2));
    machine.connection_request().unwrap();
    assert_eq!(
        machine.push(b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html>"),
        vec![NtripEvent::Rejected(
            NtripRejection::UnexpectedContentType {
                content_type: "text/html".into(),
            }
        )]
    );
}

#[test]
fn sourcetable_round_trip_preserves_unknown_and_raw_fields() {
    let text = "STR;MP;ID;RTCM;details;bad;GPS;NET;USA;badlat;1.0;2;0;gen;none;Z;maybe;badrate;tail;more\r\nXYZ;a;b\r\nENDSOURCETABLE\r\n";
    let table = parse_sourcetable(text).unwrap();
    let reparsed = parse_sourcetable(&table.to_text()).unwrap();
    assert_eq!(reparsed, table);

    let stream = table.streams().next().unwrap();
    assert_eq!(stream.carrier, Field::Raw("bad".into()));
    assert_eq!(stream.lat_deg, Field::Raw("badlat".into()));
    assert_eq!(stream.misc, "tail;more");
}

#[test]
fn gga_formatter_and_machine_pacing_are_deterministic() {
    let position = GgaPosition {
        lat_deg: 40.0,
        lon_deg: -105.0,
        height_m: 1600.0,
        ..GgaPosition::default()
    };
    let sentence = format_gga(&position, 3661.239).unwrap();
    assert_eq!(
        String::from_utf8(sentence).unwrap(),
        "$GPGGA,010101.23,4000.0000000,N,10500.0000000,W,1,10,1.0,1600.000,M,,M,,*57\r\n"
    );

    let mut cfg = config(NtripVersion::Rev1);
    cfg.gga_interval_s = Some(10.0);
    let mut machine = NtripClientMachine::new(cfg);
    machine.connection_request().unwrap();
    machine.push(b"ICY 200 OK\r\n");
    assert!(machine.gga_message(5.0, &position, 1.0).is_some());
    assert!(machine.gga_message(14.0, &position, 2.0).is_none());
    assert!(machine.gga_message(15.0, &position, 3.0).is_some());
    assert!(machine.gga_message(14.0, &position, 4.0).is_none());
}

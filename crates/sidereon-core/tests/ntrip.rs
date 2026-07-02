use sidereon_core::ntrip::{
    NtripClientMachine, NtripConfig, NtripEvent, NtripHandshake, NtripVersion,
};
use sidereon_core::rtcm::{self, Message, SsrStreamAssembler};

#[test]
fn ntrip_machine_payload_feeds_rtcm_assembler() {
    let frame = rtcm::encode_frame(&[0xff, 0xf0]).unwrap();
    let mut wire =
        b"HTTP/1.1 200 OK\r\nContent-Type: gnss/data\r\nTransfer-Encoding: chunked\r\n\r\n"
            .to_vec();
    wire.extend_from_slice(format!("{:X}\r\n", frame.len()).as_bytes());
    wire.extend_from_slice(&frame);
    wire.extend_from_slice(b"\r\n0\r\n\r\n");

    let mut machine = NtripClientMachine::new(NtripConfig {
        host: "caster.example.test".into(),
        port: 2101,
        mountpoint: "MOUNT".into(),
        version: NtripVersion::Rev2,
        credentials: None,
        user_agent_product: "sidereon-test/0".into(),
        gga_interval_s: None,
    });
    machine.connection_request().unwrap();
    let events = machine.push(&wire);
    assert!(matches!(
        &events[0],
        NtripEvent::Connected(NtripHandshake {
            version: NtripVersion::Rev2,
            chunked: true,
            ..
        })
    ));

    let mut assembler = SsrStreamAssembler::new();
    let messages: Vec<_> = events
        .into_iter()
        .filter_map(|event| match event {
            NtripEvent::Payload(bytes) => Some(bytes),
            _ => None,
        })
        .flat_map(|bytes| assembler.push(&bytes))
        .collect();
    assert!(matches!(
        &messages[0],
        Ok(Message::Unsupported(message)) if message.message_number == 4095
    ));
}

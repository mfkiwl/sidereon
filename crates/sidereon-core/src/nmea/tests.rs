use super::*;

const GGA_SAMPLE: &str = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n";

#[test]
fn parses_gga_sentence_fields() {
    let parsed = parse_sentence(GGA_SAMPLE).expect("parse GGA");
    assert!(parsed.diagnostics.is_empty());
    assert_eq!(
        parsed.value.talker,
        NmeaTalker::System(crate::GnssSystem::Gps)
    );
    let NmeaBody::Gga(gga) = parsed.value.body;
    assert_eq!(
        gga.time,
        Some(NmeaTime {
            hour: 12,
            minute: 35,
            second: 19,
            nanos: 0,
            decimals: 0,
        })
    );
    assert_eq!(gga.quality, Some(GgaQuality::GpsSps));
    assert_eq!(gga.satellites_used, Some(8));
    assert_eq!(gga.hdop, Some(0.9));
    assert_eq!(gga.altitude_msl_m, Some(545.4));
    assert_eq!(gga.geoid_separation_m, Some(46.9));
    assert_eq!(gga.latitude.expect("latitude").degrees, 48);
    assert_eq!(gga.longitude.expect("longitude").degrees, 11);
}

#[test]
fn rejects_checksum_mismatch_before_decoding() {
    let error = parse_sentence("$GPGGA,123519,4807.038,N,01131.000,E,9,08,0.9,545.4,M,46.9,M,,*47")
        .expect_err("checksum mismatch");
    assert!(matches!(error, NmeaError::ChecksumMismatch { .. }));

    let parsed =
        parse_nmea_str("$GPGGA,123519,4807.038,N,01131.000,E,9,08,0.9,545.4,M,46.9,M,,*47\n");
    assert!(parsed.value.sentences.is_empty());
    assert_eq!(
        parsed.diagnostics.skips[0].reason,
        SkipReason::InconsistentRecord("checksum mismatch")
    );
}

#[test]
fn unsupported_and_proprietary_sentences_are_typed_skips() {
    let parsed = parse_nmea_str("$GPRMC,123519,A,,,,,,,,,,*07\n$PUBX,00*33\n");
    assert!(parsed.value.sentences.is_empty());
    assert_eq!(parsed.diagnostics.skips.len(), 2);
    assert_eq!(
        parsed.diagnostics.skips[0].reason,
        SkipReason::UnsupportedRecordType("unsupported sentence type")
    );
    assert_eq!(
        parsed.diagnostics.skips[1].reason,
        SkipReason::UnsupportedRecordType("proprietary sentence")
    );
}

#[test]
fn missing_checksum_decodes_with_warning() {
    let parsed = parse_sentence("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,")
        .expect("parse without checksum");
    assert_eq!(parsed.diagnostics.warnings.len(), 1);
    assert_eq!(
        parsed.diagnostics.warnings[0].kind,
        WarningKind::MissingMetadata
    );
}

#[test]
fn write_gga_round_trips_and_emits_checksum() {
    let gga = Gga {
        time: Some(NmeaTime {
            hour: 12,
            minute: 35,
            second: 19,
            nanos: 0,
            decimals: 0,
        }),
        latitude: Some(NmeaCoordinate::parse("4807.038", "N", true).unwrap()),
        longitude: Some(NmeaCoordinate::parse("01131.000", "E", false).unwrap()),
        quality: Some(GgaQuality::GpsSps),
        satellites_used: Some(8),
        hdop: Some(0.9),
        altitude_msl_m: Some(545.4),
        geoid_separation_m: Some(46.9),
        differential_age_s: None,
        differential_station_id: None,
    };
    let sentence = write_gga(NmeaTalker::System(crate::GnssSystem::Gps), &gga).unwrap();
    assert_eq!(
        sentence,
        "$GPGGA,123519.00,4807.038,N,01131.000,E,1,08,0.90,545.4,M,46.9,M,,*59\r\n"
    );
    let round_trip = parse_sentence(&sentence).expect("parse written GGA");
    let NmeaBody::Gga(parsed_gga) = round_trip.value.body;
    assert_eq!(
        parsed_gga.time,
        gga.time.map(|mut time| {
            time.decimals = 2;
            time
        })
    );
    assert_eq!(parsed_gga.latitude, gga.latitude);
    assert_eq!(parsed_gga.longitude, gga.longitude);
    assert_eq!(
        write_gga(round_trip.value.talker, &parsed_gga).unwrap(),
        sentence
    );
}

#[test]
fn accumulator_splits_gga_epochs_by_exact_time() {
    let log = parse_nmea_str(
        "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\n\
         $GPGGA,123520,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*4D\n",
    );
    assert!(log.diagnostics.is_empty());
    let epochs = group_epochs(&log.value);
    assert_eq!(epochs.len(), 2);
    assert_eq!(epochs[0].time_of_day.expect("time").second, 19);
    assert_eq!(epochs[1].time_of_day.expect("time").second, 20);
}

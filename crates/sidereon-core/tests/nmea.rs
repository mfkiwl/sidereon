use sidereon_core::nmea::{self, NmeaBody};

#[test]
fn nmea_integration_groups_mixed_epoch() {
    let log = nmea::parse_nmea_str(
        "$GPRMC,123520,A,4807.038,N,01131.000,E,22.4,84.4,230394,3.1,W,A,S*72\n\
         $GNGSA,A,3,01,02,03,04,,,,,,,,,1.5,0.9,1.2,1*3B\n\
         $GPGSV,1,1,01,01,45,083,42,1*58\n\
         $GPGGA,123521,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*4C\n",
    );
    assert!(log.diagnostics.skips.is_empty(), "{:?}", log.diagnostics);
    assert!(matches!(log.value.sentences[0].body, NmeaBody::Rmc(_)));

    let epochs = nmea::group_epochs(&log.value);
    assert_eq!(epochs.len(), 2);
    assert_eq!(epochs[0].time_of_day.unwrap().second, 20);
    assert_eq!(epochs[0].date.unwrap().year, 1994);
    assert_eq!(epochs[0].gsa.len(), 1);
    assert_eq!(epochs[0].gsv[0].satellites.len(), 1);
    assert_eq!(epochs[1].time_of_day.unwrap().second, 21);
}

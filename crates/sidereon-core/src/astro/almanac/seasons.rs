use crate::astro::almanac::{
    body_ecliptic, find_angle_crossing_times, validate_scan_controls, AlmanacError,
    EphemerisSource, SeasonEvent, SeasonKind, NAIF_SUN, SEASON_PLANET_STEP_MAX_SECONDS,
};
use crate::astro::passes::UtcInstant;

/// Find equinoxes and solstices in ascending time order.
pub fn seasons(
    source: EphemerisSource<'_>,
    start: UtcInstant,
    end: UtcInstant,
    step_seconds: f64,
    time_tolerance_seconds: f64,
) -> Result<Vec<SeasonEvent>, AlmanacError> {
    validate_scan_controls(
        step_seconds,
        time_tolerance_seconds,
        SEASON_PLANET_STEP_MAX_SECONDS,
    )?;

    let mut events = Vec::new();
    for (index, kind) in [
        SeasonKind::MarchEquinox,
        SeasonKind::JuneSolstice,
        SeasonKind::SeptemberEquinox,
        SeasonKind::DecemberSolstice,
    ]
    .into_iter()
    .enumerate()
    {
        let target_deg = 90.0 * index as f64;
        let times = find_angle_crossing_times(
            start,
            end,
            step_seconds,
            time_tolerance_seconds,
            target_deg,
            |time| Ok(body_ecliptic(source, NAIF_SUN, time)?.longitude_deg),
        )?;
        events.extend(times.into_iter().map(|time| SeasonEvent { time, kind }));
    }
    events.sort_by_key(|event| event.time);
    Ok(events)
}

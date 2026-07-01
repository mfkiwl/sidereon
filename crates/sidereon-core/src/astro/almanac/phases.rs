use crate::astro::almanac::{
    body_ecliptic, find_angle_crossing_times, validate_scan_controls, wrap360, AlmanacError,
    EphemerisSource, MoonPhaseEvent, MoonPhaseKind, NAIF_MOON, NAIF_SUN, PHASE_STEP_MAX_SECONDS,
};
use crate::astro::passes::UtcInstant;

/// Find principal lunar phases in ascending time order.
pub fn moon_phases(
    source: EphemerisSource<'_>,
    start: UtcInstant,
    end: UtcInstant,
    step_seconds: f64,
    time_tolerance_seconds: f64,
) -> Result<Vec<MoonPhaseEvent>, AlmanacError> {
    validate_scan_controls(step_seconds, time_tolerance_seconds, PHASE_STEP_MAX_SECONDS)?;

    let mut events = Vec::new();
    for (index, kind) in [
        MoonPhaseKind::New,
        MoonPhaseKind::FirstQuarter,
        MoonPhaseKind::Full,
        MoonPhaseKind::LastQuarter,
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
            |time| moon_phase_deg(source, time),
        )?;
        events.extend(times.into_iter().map(|time| MoonPhaseEvent { time, kind }));
    }
    events.sort_by_key(|event| event.time);
    Ok(events)
}

/// Continuous Moon phase angle, `wrap360(L_moon - L_sun)`, degrees.
pub fn moon_phase_deg(source: EphemerisSource<'_>, at: UtcInstant) -> Result<f64, AlmanacError> {
    let moon = body_ecliptic(source, NAIF_MOON, at)?;
    let sun = body_ecliptic(source, NAIF_SUN, at)?;
    Ok(wrap360(moon.longitude_deg - sun.longitude_deg))
}

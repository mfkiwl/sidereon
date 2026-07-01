use crate::astro::almanac::{
    body_ecliptic, find_angle_crossing_times, is_inferior, planet_naif, validate_scan_controls,
    wrap360, AlmanacError, EphemerisSource, Planet, PlanetaryEvent, PlanetaryEventKind, NAIF_SUN,
    SEASON_PLANET_STEP_MAX_SECONDS,
};
use crate::astro::passes::UtcInstant;

/// Find ecliptic-longitude conjunctions or oppositions for a planet.
pub fn planetary_events(
    source: EphemerisSource<'_>,
    planet: Planet,
    kind: PlanetaryEventKind,
    start: UtcInstant,
    end: UtcInstant,
    step_seconds: f64,
    time_tolerance_seconds: f64,
) -> Result<Vec<PlanetaryEvent>, AlmanacError> {
    validate_scan_controls(
        step_seconds,
        time_tolerance_seconds,
        SEASON_PLANET_STEP_MAX_SECONDS,
    )?;
    if matches!(source, EphemerisSource::Analytic) {
        return Err(AlmanacError::EphemerisRequired);
    }
    if kind == PlanetaryEventKind::Opposition && is_inferior(planet) {
        return Err(AlmanacError::InferiorPlanetOpposition);
    }

    let target_deg = match kind {
        PlanetaryEventKind::Conjunction => 0.0,
        PlanetaryEventKind::Opposition => 180.0,
    };
    let planet_naif = planet_naif(planet);
    let times = find_angle_crossing_times(
        start,
        end,
        step_seconds,
        time_tolerance_seconds,
        target_deg,
        |time| planet_sun_delta_deg(source, planet_naif, time),
    )?;

    times
        .into_iter()
        .map(|time| {
            let elongation_deg = wrap360(planet_sun_delta_deg(source, planet_naif, time)?);
            Ok(PlanetaryEvent {
                time,
                planet,
                kind,
                elongation_deg,
            })
        })
        .collect()
}

fn planet_sun_delta_deg(
    source: EphemerisSource<'_>,
    planet_naif: i32,
    time: UtcInstant,
) -> Result<f64, AlmanacError> {
    let planet = body_ecliptic(source, planet_naif, time)?;
    let sun = body_ecliptic(source, NAIF_SUN, time)?;
    Ok(planet.longitude_deg - sun.longitude_deg)
}

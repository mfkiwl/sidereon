//! WGS84 geodesic direct and inverse helpers.
//!
//! The computations use Karney's geodesic algorithm on the WGS84 ellipsoid.
//! Public angles are degrees and distances are meters.

use oxiproj_geodesic::Geodesic;

/// Error returned when geodesic inputs are outside the accepted domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GeodesicError {
    /// A geodesic input was non-finite or outside its documented range.
    #[error("invalid geodesic input {field}: {reason}")]
    InvalidInput {
        /// Name of the invalid field.
        field: &'static str,
        /// Reason the field was rejected.
        reason: &'static str,
    },
}

fn invalid_input(field: &'static str, reason: &'static str) -> GeodesicError {
    GeodesicError::InvalidInput { field, reason }
}

fn validate_latitude(field: &'static str, value: f64) -> Result<(), GeodesicError> {
    if !value.is_finite() {
        return Err(invalid_input(field, "must be finite"));
    }
    if !(-90.0..=90.0).contains(&value) {
        return Err(invalid_input(field, "must be in [-90, 90] degrees"));
    }
    Ok(())
}

fn validate_finite(field: &'static str, value: f64) -> Result<(), GeodesicError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid_input(field, "must be finite"))
    }
}

/// Solve the WGS84 inverse geodesic problem.
///
/// Inputs are point 1 latitude and longitude followed by point 2 latitude and
/// longitude, all in degrees. Longitudes may be outside `[-180, 180]`; they are
/// reduced by the geodesic solver. The returned tuple is `(s12_m, azi1_deg,
/// azi2_deg)`, where `s12_m` is the geodesic distance in meters, `azi1_deg` is
/// the forward azimuth at point 1, and `azi2_deg` is the forward azimuth at
/// point 2.
pub fn geodesic_inverse(
    lat1_deg: f64,
    lon1_deg: f64,
    lat2_deg: f64,
    lon2_deg: f64,
) -> Result<(f64, f64, f64), GeodesicError> {
    validate_latitude("lat1_deg", lat1_deg)?;
    validate_finite("lon1_deg", lon1_deg)?;
    validate_latitude("lat2_deg", lat2_deg)?;
    validate_finite("lon2_deg", lon2_deg)?;

    let geodesic = Geodesic::wgs84();
    let result = geodesic.inverse(lat1_deg, lon1_deg, lat2_deg, lon2_deg);
    Ok((result.s12, result.azi1, result.azi2))
}

/// Solve the WGS84 direct geodesic problem.
///
/// Inputs are point 1 latitude, longitude, forward azimuth, and geodesic
/// distance. Angles are degrees and `s12_m` is meters. Longitudes and azimuths
/// may be outside one turn; they are reduced by the geodesic solver. The
/// returned tuple is `(lat2_deg, lon2_deg, azi2_deg)`.
pub fn geodesic_direct(
    lat1_deg: f64,
    lon1_deg: f64,
    azi1_deg: f64,
    s12_m: f64,
) -> Result<(f64, f64, f64), GeodesicError> {
    validate_latitude("lat1_deg", lat1_deg)?;
    validate_finite("lon1_deg", lon1_deg)?;
    validate_finite("azi1_deg", azi1_deg)?;
    validate_finite("s12_m", s12_m)?;

    let geodesic = Geodesic::wgs84();
    let result = geodesic.direct(lat1_deg, lon1_deg, azi1_deg, s12_m);
    Ok((result.lat2, result.lon2, result.azi2))
}

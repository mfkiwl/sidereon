//! IMU samples, calibration, and pre-mechanization correction.

use crate::astro::math::mat3::{mul_vec3, Mat3};
use crate::astro::math::vec3::{scale3, sub3};

use super::{invalid_input, validate_finite, validate_mat3, validate_vec3, InertialError};

/// One IMU sample tagged by its end time in seconds since J2000.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImuSample {
    /// Sample end time in seconds since J2000 on the caller's GNSS time scale.
    pub t_j2000_s: f64,
    /// Sample payload as rates or sensor-provided increments.
    pub kind: ImuSampleKind,
}

impl ImuSample {
    /// Build an IMU sample from rates.
    pub const fn rate(
        t_j2000_s: f64,
        specific_force_mps2: [f64; 3],
        angular_rate_rps: [f64; 3],
    ) -> Self {
        Self {
            t_j2000_s,
            kind: ImuSampleKind::Rate {
                specific_force_mps2,
                angular_rate_rps,
            },
        }
    }

    /// Build an IMU sample from increments.
    pub const fn increment(
        t_j2000_s: f64,
        delta_velocity_mps: [f64; 3],
        delta_theta_rad: [f64; 3],
        dt_s: f64,
    ) -> Self {
        Self {
            t_j2000_s,
            kind: ImuSampleKind::Increment {
                delta_velocity_mps,
                delta_theta_rad,
                dt_s,
            },
        }
    }
}

/// IMU sample payload.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImuSampleKind {
    /// Specific force and angular rate sampled over the interval ending at the
    /// sample time.
    Rate {
        /// Specific force in body axes, m/s^2.
        specific_force_mps2: [f64; 3],
        /// Angular rate in body axes, rad/s.
        angular_rate_rps: [f64; 3],
    },
    /// Sensor-provided increments over `dt_s`.
    Increment {
        /// Body-frame delta velocity in m/s.
        delta_velocity_mps: [f64; 3],
        /// Body-frame delta angle in rad.
        delta_theta_rad: [f64; 3],
        /// Sample integration interval in seconds.
        dt_s: f64,
    },
}

/// Accelerometer and gyroscope biases.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImuBias {
    /// Accelerometer bias in m/s^2.
    pub accel_mps2: [f64; 3],
    /// Gyroscope bias in rad/s.
    pub gyro_rps: [f64; 3],
}

impl Default for ImuBias {
    fn default() -> Self {
        Self {
            accel_mps2: [0.0; 3],
            gyro_rps: [0.0; 3],
        }
    }
}

impl ImuBias {
    /// Validate finite bias components.
    pub fn validate(&self) -> Result<(), InertialError> {
        validate_vec3(self.accel_mps2, "accel_mps2")?;
        validate_vec3(self.gyro_rps, "gyro_rps")
    }
}

/// IMU scale-factor and misalignment matrices.
///
/// Each matrix is the `M` in `(I + M)^-1(measured - bias)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImuCalibration {
    /// Accelerometer scale and misalignment matrix.
    pub accel_scale_misalignment: Mat3,
    /// Gyroscope scale and misalignment matrix.
    pub gyro_scale_misalignment: Mat3,
}

impl Default for ImuCalibration {
    fn default() -> Self {
        Self {
            accel_scale_misalignment: [[0.0; 3]; 3],
            gyro_scale_misalignment: [[0.0; 3]; 3],
        }
    }
}

impl ImuCalibration {
    /// Build a diagonal calibration from scale factors in parts per million.
    pub fn from_scale_ppm(
        accel_scale_ppm: [f64; 3],
        gyro_scale_ppm: [f64; 3],
    ) -> Result<Self, InertialError> {
        validate_vec3(accel_scale_ppm, "accel_scale_ppm")?;
        validate_vec3(gyro_scale_ppm, "gyro_scale_ppm")?;
        let mut accel = [[0.0; 3]; 3];
        let mut gyro = [[0.0; 3]; 3];
        for i in 0..3 {
            accel[i][i] = accel_scale_ppm[i] * 1.0e-6;
            gyro[i][i] = gyro_scale_ppm[i] * 1.0e-6;
        }
        Ok(Self {
            accel_scale_misalignment: accel,
            gyro_scale_misalignment: gyro,
        })
    }

    /// Validate finite calibration matrices and invertibility of `I + M`.
    pub fn validate(&self) -> Result<(), InertialError> {
        validate_mat3(&self.accel_scale_misalignment, "accel_scale_misalignment")?;
        validate_mat3(&self.gyro_scale_misalignment, "gyro_scale_misalignment")?;
        inverse3(&identity_plus(&self.accel_scale_misalignment))?;
        inverse3(&identity_plus(&self.gyro_scale_misalignment))?;
        Ok(())
    }
}

/// Bias and calibration model applied before strapdown mechanization.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ImuErrorModel {
    /// Biases subtracted before scale and misalignment correction.
    pub bias: ImuBias,
    /// Scale-factor and misalignment correction.
    pub calibration: ImuCalibration,
}

impl ImuErrorModel {
    /// Correct a raw IMU sample into a body-frame increment.
    pub fn correct_sample(
        &self,
        sample: &ImuSample,
        previous_t_j2000_s: f64,
    ) -> Result<CorrectedImuIncrement, InertialError> {
        validate_finite(previous_t_j2000_s, "previous_t_j2000_s")?;
        validate_finite(sample.t_j2000_s, "t_j2000_s")?;
        self.bias.validate()?;
        self.calibration.validate()?;
        let epoch_dt_s = sample.t_j2000_s - previous_t_j2000_s;
        if epoch_dt_s <= 0.0 {
            return Err(InertialError::NonMonotonicSample);
        }

        match sample.kind {
            ImuSampleKind::Rate {
                specific_force_mps2,
                angular_rate_rps,
            } => {
                validate_vec3(specific_force_mps2, "specific_force_mps2")?;
                validate_vec3(angular_rate_rps, "angular_rate_rps")?;
                let f_corr = correct_rate_vector(
                    specific_force_mps2,
                    self.bias.accel_mps2,
                    &self.calibration.accel_scale_misalignment,
                )?;
                let omega_corr = correct_rate_vector(
                    angular_rate_rps,
                    self.bias.gyro_rps,
                    &self.calibration.gyro_scale_misalignment,
                )?;
                Ok(CorrectedImuIncrement {
                    t_j2000_s: sample.t_j2000_s,
                    delta_velocity_mps: scale3(f_corr, epoch_dt_s),
                    delta_theta_rad: scale3(omega_corr, epoch_dt_s),
                    dt_s: epoch_dt_s,
                })
            }
            ImuSampleKind::Increment {
                delta_velocity_mps,
                delta_theta_rad,
                dt_s,
            } => {
                validate_vec3(delta_velocity_mps, "delta_velocity_mps")?;
                validate_vec3(delta_theta_rad, "delta_theta_rad")?;
                validate_finite(dt_s, "dt_s")?;
                if dt_s <= 0.0 {
                    return Err(invalid_input("dt_s", "must be positive"));
                }
                let tolerance = 1.0e-12_f64.max(1.0e-12 * dt_s.abs());
                if (epoch_dt_s - dt_s).abs() > tolerance {
                    return Err(invalid_input("dt_s", "must match sample time step"));
                }
                let f_meas = scale3(delta_velocity_mps, 1.0 / dt_s);
                let omega_meas = scale3(delta_theta_rad, 1.0 / dt_s);
                let f_corr = correct_rate_vector(
                    f_meas,
                    self.bias.accel_mps2,
                    &self.calibration.accel_scale_misalignment,
                )?;
                let omega_corr = correct_rate_vector(
                    omega_meas,
                    self.bias.gyro_rps,
                    &self.calibration.gyro_scale_misalignment,
                )?;
                Ok(CorrectedImuIncrement {
                    t_j2000_s: sample.t_j2000_s,
                    delta_velocity_mps: scale3(f_corr, dt_s),
                    delta_theta_rad: scale3(omega_corr, dt_s),
                    dt_s,
                })
            }
        }
    }
}

/// Bias-corrected, scale-corrected IMU increment consumed by mechanization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorrectedImuIncrement {
    /// Sample end time in seconds since J2000.
    pub t_j2000_s: f64,
    /// Corrected body-frame delta velocity in m/s.
    pub delta_velocity_mps: [f64; 3],
    /// Corrected body-frame delta angle in rad.
    pub delta_theta_rad: [f64; 3],
    /// Sample integration interval in seconds.
    pub dt_s: f64,
}

fn correct_rate_vector(
    measured: [f64; 3],
    bias: [f64; 3],
    scale_misalignment: &Mat3,
) -> Result<[f64; 3], InertialError> {
    let inverse = inverse3(&identity_plus(scale_misalignment))?;
    Ok(mul_vec3(&inverse, sub3(measured, bias)))
}

fn identity_plus(m: &Mat3) -> Mat3 {
    [
        [1.0 + m[0][0], m[0][1], m[0][2]],
        [m[1][0], 1.0 + m[1][1], m[1][2]],
        [m[2][0], m[2][1], 1.0 + m[2][2]],
    ]
}

fn inverse3(m: &Mat3) -> Result<Mat3, InertialError> {
    let c00 = m[1][1] * m[2][2] - m[1][2] * m[2][1];
    let c01 = -(m[1][0] * m[2][2] - m[1][2] * m[2][0]);
    let c02 = m[1][0] * m[2][1] - m[1][1] * m[2][0];
    let c10 = -(m[0][1] * m[2][2] - m[0][2] * m[2][1]);
    let c11 = m[0][0] * m[2][2] - m[0][2] * m[2][0];
    let c12 = -(m[0][0] * m[2][1] - m[0][1] * m[2][0]);
    let c20 = m[0][1] * m[1][2] - m[0][2] * m[1][1];
    let c21 = -(m[0][0] * m[1][2] - m[0][2] * m[1][0]);
    let c22 = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    let det = m[0][0] * c00 + m[0][1] * c01 + m[0][2] * c02;
    if !det.is_finite() || det == 0.0 {
        return Err(InertialError::SingularCalibration);
    }
    let inv_det = 1.0 / det;
    Ok([
        [c00 * inv_det, c10 * inv_det, c20 * inv_det],
        [c01 * inv_det, c11 * inv_det, c21 * inv_det],
        [c02 * inv_det, c12 * inv_det, c22 * inv_det],
    ])
}

#[cfg(test)]
mod tests {
    //! Provenance: IMU correction follows Groves, Principles of GNSS,
    //! Inertial, and Multisensor Integrated Navigation Systems, 2nd ed.,
    //! Chapter 4.4, using `(I + M)^-1(measured - bias)` before
    //! mechanization.

    use super::*;

    #[test]
    fn increment_correction_applies_bias_and_scale_exact_formula() {
        let calibration = ImuCalibration::from_scale_ppm([100.0, -50.0, 25.0], [10.0, -20.0, 40.0])
            .expect("calibration");
        let model = ImuErrorModel {
            bias: ImuBias {
                accel_mps2: [0.1, -0.2, 0.05],
                gyro_rps: [0.01, -0.02, 0.03],
            },
            calibration,
        };
        let sample = ImuSample::increment(11.0, [1.1, -2.4, 0.7], [0.11, -0.24, 0.37], 1.0);
        let corrected = model.correct_sample(&sample, 10.0).expect("corrected");

        let expected_accel_x = (1.1_f64 - 0.1) / (1.0 + 100.0e-6);
        let expected_accel_y = (-2.4_f64 + 0.2) / (1.0 - 50.0e-6);
        let expected_accel_z = (0.7_f64 - 0.05) / (1.0 + 25.0e-6);
        assert_eq!(
            corrected.delta_velocity_mps[0].to_bits(),
            expected_accel_x.to_bits()
        );
        assert!(
            (corrected.delta_velocity_mps[1] - expected_accel_y).abs() <= 5.0e-16,
            "actual {:.17e}, expected {:.17e}",
            corrected.delta_velocity_mps[1],
            expected_accel_y
        );
        assert_eq!(
            corrected.delta_velocity_mps[2].to_bits(),
            expected_accel_z.to_bits()
        );

        let expected_gyro_x = (0.11_f64 - 0.01) / (1.0 + 10.0e-6);
        let expected_gyro_y = (-0.24_f64 + 0.02) / (1.0 - 20.0e-6);
        let expected_gyro_z = (0.37_f64 - 0.03) / (1.0 + 40.0e-6);
        assert_eq!(
            corrected.delta_theta_rad[0].to_bits(),
            expected_gyro_x.to_bits()
        );
        assert!(
            (corrected.delta_theta_rad[1] - expected_gyro_y).abs() <= 1.0e-16,
            "actual {:.17e}, expected {:.17e}",
            corrected.delta_theta_rad[1],
            expected_gyro_y
        );
        assert!(
            (corrected.delta_theta_rad[2] - expected_gyro_z).abs() <= 1.0e-16,
            "actual {:.17e}, expected {:.17e}",
            corrected.delta_theta_rad[2],
            expected_gyro_z
        );
    }

    #[test]
    fn singular_calibration_is_flagged() {
        let calibration = ImuCalibration {
            accel_scale_misalignment: [[-1.0, 0.0, 0.0], [0.0; 3], [0.0; 3]],
            gyro_scale_misalignment: [[0.0; 3]; 3],
        };
        assert_eq!(
            calibration.validate(),
            Err(InertialError::SingularCalibration)
        );
    }
}

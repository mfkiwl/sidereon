//! Epoch-aware terrestrial reference-frame transformations.
//!
//! The catalog contains published 14-parameter Helmert transforms between
//! ITRF/ETRF realizations. Epochs are decimal years, positions are geocentric
//! Cartesian metres, and station velocities are metres per year.

use core::fmt;

use crate::astro::math::linear::invert_3x3_adjugate;
use crate::astro::math::mat3::{mul_vec3, Mat3};
use crate::astro::math::vec3::{add3, scale3, sub3};
use crate::frame::ItrfPositionM;

const MM_TO_M: f64 = 1.0e-3;
const PPB_TO_SCALE: f64 = 1.0e-9;
const MAS_TO_RAD: f64 = core::f64::consts::PI / (180.0 * 3600.0 * 1000.0);

/// A supported terrestrial reference-frame realization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerrestrialFrame {
    /// International Terrestrial Reference Frame 2020.
    Itrf2020,
    /// International Terrestrial Reference Frame 2014.
    Itrf2014,
    /// International Terrestrial Reference Frame 2008.
    Itrf2008,
    /// European Terrestrial Reference Frame 2020.
    Etrf2020,
}

impl TerrestrialFrame {
    fn index(self) -> usize {
        match self {
            Self::Itrf2020 => 0,
            Self::Itrf2014 => 1,
            Self::Itrf2008 => 2,
            Self::Etrf2020 => 3,
        }
    }
}

impl fmt::Display for TerrestrialFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Itrf2020 => f.write_str("ITRF2020"),
            Self::Itrf2014 => f.write_str("ITRF2014"),
            Self::Itrf2008 => f.write_str("ITRF2008"),
            Self::Etrf2020 => f.write_str("ETRF2020"),
        }
    }
}

/// Cartesian position in a named terrestrial frame, in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrestrialPositionM {
    /// Geocentric X coordinate in metres.
    pub x_m: f64,
    /// Geocentric Y coordinate in metres.
    pub y_m: f64,
    /// Geocentric Z coordinate in metres.
    pub z_m: f64,
}

impl TerrestrialPositionM {
    /// Construct a finite Cartesian terrestrial position in metres.
    pub const fn new(x_m: f64, y_m: f64, z_m: f64) -> Result<Self, FrameCatalogError> {
        if !x_m.is_finite() {
            return Err(invalid_input("x_m", "must be finite"));
        }
        if !y_m.is_finite() {
            return Err(invalid_input("y_m", "must be finite"));
        }
        if !z_m.is_finite() {
            return Err(invalid_input("z_m", "must be finite"));
        }
        Ok(Self { x_m, y_m, z_m })
    }

    /// Construct a finite Cartesian terrestrial position from `[x, y, z]`.
    pub const fn from_array(position_m: [f64; 3]) -> Result<Self, FrameCatalogError> {
        Self::new(position_m[0], position_m[1], position_m[2])
    }

    /// Return the Cartesian components as `[x, y, z]` metres.
    pub const fn as_array(self) -> [f64; 3] {
        [self.x_m, self.y_m, self.z_m]
    }

    /// Convert a frame-tagged ITRF position into the catalog position type.
    pub const fn from_itrf(position: ItrfPositionM) -> Self {
        Self {
            x_m: position.x_m,
            y_m: position.y_m,
            z_m: position.z_m,
        }
    }
}

impl From<ItrfPositionM> for TerrestrialPositionM {
    fn from(position: ItrfPositionM) -> Self {
        Self::from_itrf(position)
    }
}

/// Cartesian station velocity in a named terrestrial frame, in metres per year.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrestrialVelocityMPerYear {
    /// Geocentric X velocity in metres per year.
    pub vx_m_per_year: f64,
    /// Geocentric Y velocity in metres per year.
    pub vy_m_per_year: f64,
    /// Geocentric Z velocity in metres per year.
    pub vz_m_per_year: f64,
}

impl TerrestrialVelocityMPerYear {
    /// Construct a finite Cartesian station velocity in metres per year.
    pub const fn new(
        vx_m_per_year: f64,
        vy_m_per_year: f64,
        vz_m_per_year: f64,
    ) -> Result<Self, FrameCatalogError> {
        if !vx_m_per_year.is_finite() {
            return Err(invalid_input("vx_m_per_year", "must be finite"));
        }
        if !vy_m_per_year.is_finite() {
            return Err(invalid_input("vy_m_per_year", "must be finite"));
        }
        if !vz_m_per_year.is_finite() {
            return Err(invalid_input("vz_m_per_year", "must be finite"));
        }
        Ok(Self {
            vx_m_per_year,
            vy_m_per_year,
            vz_m_per_year,
        })
    }

    /// Construct a finite Cartesian station velocity from `[vx, vy, vz]`.
    pub const fn from_array(velocity_m_per_year: [f64; 3]) -> Result<Self, FrameCatalogError> {
        Self::new(
            velocity_m_per_year[0],
            velocity_m_per_year[1],
            velocity_m_per_year[2],
        )
    }

    /// Return the Cartesian components as `[vx, vy, vz]` metres per year.
    pub const fn as_array(self) -> [f64; 3] {
        [self.vx_m_per_year, self.vy_m_per_year, self.vz_m_per_year]
    }
}

/// A transformed terrestrial position and optional station velocity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrestrialState {
    /// Transformed Cartesian position in the target frame.
    pub position: TerrestrialPositionM,
    /// Transformed station velocity in the target frame, when supplied.
    pub velocity: Option<TerrestrialVelocityMPerYear>,
}

/// Helmert parameters in the units used by the published tables.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HelmertParameters {
    /// Translation components `[Tx, Ty, Tz]`, in millimetres.
    pub translation_mm: [f64; 3],
    /// Scale difference `D`, in parts per billion.
    pub scale_ppb: f64,
    /// Rotation components `[Rx, Ry, Rz]`, in milliarcseconds.
    pub rotation_mas: [f64; 3],
}

/// Helmert parameter rates in the units used by the published tables.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HelmertRates {
    /// Translation rates `[Tx, Ty, Tz]`, in millimetres per year.
    pub translation_mm_per_year: [f64; 3],
    /// Scale rate `D`, in parts per billion per year.
    pub scale_ppb_per_year: f64,
    /// Rotation rates `[Rx, Ry, Rz]`, in milliarcseconds per year.
    pub rotation_mas_per_year: [f64; 3],
}

/// One published 14-parameter Helmert catalog entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HelmertTransform {
    /// Source frame for the published forward transform.
    pub from: TerrestrialFrame,
    /// Target frame for the published forward transform.
    pub to: TerrestrialFrame,
    /// Parameter reference epoch, expressed as a decimal year.
    pub reference_epoch_year: f64,
    /// Parameters at [`reference_epoch_year`](Self::reference_epoch_year).
    pub parameters: HelmertParameters,
    /// Linear rates of the seven Helmert parameters.
    pub rates: HelmertRates,
    /// Published table or memo that supplied this entry.
    pub provenance: &'static str,
}

impl HelmertTransform {
    /// Evaluate the seven Helmert parameters at a decimal year.
    pub fn parameters_at(&self, epoch_year: f64) -> Result<HelmertParameters, FrameCatalogError> {
        validate_epoch(epoch_year)?;
        validate_finite("reference_epoch_year", self.reference_epoch_year)?;
        validate_helmert_parameters(&self.parameters)?;
        validate_helmert_rates(&self.rates)?;
        let dt_years = epoch_year - self.reference_epoch_year;
        validate_finite("epoch_delta_years", dt_years)?;
        let parameters = HelmertParameters {
            translation_mm: [
                self.parameters.translation_mm[0]
                    + self.rates.translation_mm_per_year[0] * dt_years,
                self.parameters.translation_mm[1]
                    + self.rates.translation_mm_per_year[1] * dt_years,
                self.parameters.translation_mm[2]
                    + self.rates.translation_mm_per_year[2] * dt_years,
            ],
            scale_ppb: self.parameters.scale_ppb + self.rates.scale_ppb_per_year * dt_years,
            rotation_mas: [
                self.parameters.rotation_mas[0] + self.rates.rotation_mas_per_year[0] * dt_years,
                self.parameters.rotation_mas[1] + self.rates.rotation_mas_per_year[1] * dt_years,
                self.parameters.rotation_mas[2] + self.rates.rotation_mas_per_year[2] * dt_years,
            ],
        };
        validate_helmert_parameters(&parameters)?;
        Ok(parameters)
    }

    fn evaluated(&self, epoch_year: f64) -> Result<EvaluatedHelmert, FrameCatalogError> {
        let parameters = self.parameters_at(epoch_year)?;
        Ok(EvaluatedHelmert {
            translation_m: scale3(parameters.translation_mm, MM_TO_M),
            scale: parameters.scale_ppb * PPB_TO_SCALE,
            rotation_rad: scale3(parameters.rotation_mas, MAS_TO_RAD),
            translation_rate_m_per_year: scale3(self.rates.translation_mm_per_year, MM_TO_M),
            scale_rate_per_year: self.rates.scale_ppb_per_year * PPB_TO_SCALE,
            rotation_rate_rad_per_year: scale3(self.rates.rotation_mas_per_year, MAS_TO_RAD),
        })
    }
}

/// Errors returned by terrestrial frame catalog operations.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum FrameCatalogError {
    /// An input value was non-finite or outside its documented domain.
    #[error("invalid frame catalog {field}: {reason}")]
    InvalidInput {
        /// Name of the rejected input field.
        field: &'static str,
        /// Reason the field was rejected.
        reason: &'static str,
    },
    /// No sequence of built-in catalog entries connects the requested frames.
    #[error("no terrestrial frame catalog path from {from} to {to}")]
    NoCatalogPath {
        /// Requested source frame.
        from: TerrestrialFrame,
        /// Requested target frame.
        to: TerrestrialFrame,
    },
    /// The affine Helmert matrix was singular and could not be inverted.
    #[error("singular terrestrial frame transform from {from} to {to} at epoch {epoch_year}")]
    SingularTransform {
        /// Published source frame for the transform being inverted.
        from: TerrestrialFrame,
        /// Published target frame for the transform being inverted.
        to: TerrestrialFrame,
        /// Decimal year used to evaluate the transform.
        epoch_year: f64,
    },
}

/// Built-in published terrestrial Helmert catalog entries.
pub const TERRESTRIAL_FRAME_CATALOG: &[HelmertTransform] = &[
    HelmertTransform {
        from: TerrestrialFrame::Itrf2020,
        to: TerrestrialFrame::Itrf2014,
        reference_epoch_year: 2015.0,
        parameters: HelmertParameters {
            translation_mm: [-1.4, -0.9, 1.4],
            scale_ppb: -0.42,
            rotation_mas: [0.0, 0.0, 0.0],
        },
        rates: HelmertRates {
            translation_mm_per_year: [0.0, -0.1, 0.2],
            scale_ppb_per_year: 0.0,
            rotation_mas_per_year: [0.0, 0.0, 0.0],
        },
        provenance: "ITRF/IGN Transfo-ITRF2020_TRFs.txt, ITRF2020 to past ITRFs, epoch 2015.0",
    },
    HelmertTransform {
        from: TerrestrialFrame::Itrf2020,
        to: TerrestrialFrame::Itrf2008,
        reference_epoch_year: 2015.0,
        parameters: HelmertParameters {
            translation_mm: [0.2, 1.0, 3.3],
            scale_ppb: -0.29,
            rotation_mas: [0.0, 0.0, 0.0],
        },
        rates: HelmertRates {
            translation_mm_per_year: [0.0, -0.1, 0.1],
            scale_ppb_per_year: 0.03,
            rotation_mas_per_year: [0.0, 0.0, 0.0],
        },
        provenance: "ITRF/IGN Transfo-ITRF2020_TRFs.txt, ITRF2020 to past ITRFs, epoch 2015.0",
    },
    HelmertTransform {
        from: TerrestrialFrame::Itrf2014,
        to: TerrestrialFrame::Itrf2008,
        reference_epoch_year: 2010.0,
        parameters: HelmertParameters {
            translation_mm: [1.6, 1.9, 2.4],
            scale_ppb: -0.02,
            rotation_mas: [0.0, 0.0, 0.0],
        },
        rates: HelmertRates {
            translation_mm_per_year: [0.0, 0.0, -0.1],
            scale_ppb_per_year: 0.03,
            rotation_mas_per_year: [0.0, 0.0, 0.0],
        },
        provenance: "IERS Technical Note 38, Table 2, ITRF2014 to ITRF2008, epoch 2010.0",
    },
    HelmertTransform {
        from: TerrestrialFrame::Itrf2020,
        to: TerrestrialFrame::Etrf2020,
        reference_epoch_year: 2015.0,
        parameters: HelmertParameters {
            translation_mm: [0.0, 0.0, 0.0],
            scale_ppb: 0.0,
            rotation_mas: [2.236, 13.494, -19.578],
        },
        rates: HelmertRates {
            translation_mm_per_year: [0.0, 0.0, 0.0],
            scale_ppb_per_year: 0.0,
            rotation_mas_per_year: [0.086, 0.519, -0.753],
        },
        provenance: "EUREF Technical Note 1, Table 2, ITRF2020 to ETRF2020, epoch 2015.0",
    },
];

/// Return the built-in terrestrial frame catalog.
pub fn catalog() -> &'static [HelmertTransform] {
    TERRESTRIAL_FRAME_CATALOG
}

/// Return the published catalog entry for the requested forward direction.
pub fn catalog_entry(
    from: TerrestrialFrame,
    to: TerrestrialFrame,
) -> Option<&'static HelmertTransform> {
    TERRESTRIAL_FRAME_CATALOG
        .iter()
        .find(|entry| entry.from == from && entry.to == to)
}

/// Propagate a station position from one decimal year to another.
pub fn propagate_position(
    position: TerrestrialPositionM,
    velocity: TerrestrialVelocityMPerYear,
    from_epoch_year: f64,
    to_epoch_year: f64,
) -> Result<TerrestrialPositionM, FrameCatalogError> {
    validate_epoch(from_epoch_year)?;
    validate_epoch(to_epoch_year)?;
    let position = TerrestrialPositionM::from_array(position.as_array())?;
    let velocity = TerrestrialVelocityMPerYear::from_array(velocity.as_array())?;
    let dt_years = to_epoch_year - from_epoch_year;
    let propagated = add3(position.as_array(), scale3(velocity.as_array(), dt_years));
    TerrestrialPositionM::from_array(propagated)
}

/// Propagate a station to a transform epoch, then transform it between frames.
pub fn transform_from_epoch(
    position: TerrestrialPositionM,
    velocity: TerrestrialVelocityMPerYear,
    position_epoch_year: f64,
    from: TerrestrialFrame,
    to: TerrestrialFrame,
    transform_epoch_year: f64,
) -> Result<TerrestrialState, FrameCatalogError> {
    let position_at_transform_epoch = propagate_position(
        position,
        velocity,
        position_epoch_year,
        transform_epoch_year,
    )?;
    transform(
        position_at_transform_epoch,
        Some(velocity),
        from,
        to,
        transform_epoch_year,
    )
}

/// Transform a Cartesian station position and optional velocity between frames.
pub fn transform(
    position: TerrestrialPositionM,
    velocity: Option<TerrestrialVelocityMPerYear>,
    from: TerrestrialFrame,
    to: TerrestrialFrame,
    epoch_year: f64,
) -> Result<TerrestrialState, FrameCatalogError> {
    validate_epoch(epoch_year)?;
    let position = TerrestrialPositionM::from_array(position.as_array())?;
    let velocity = velocity
        .map(|value| TerrestrialVelocityMPerYear::from_array(value.as_array()))
        .transpose()?;
    if from == to {
        return Ok(TerrestrialState { position, velocity });
    }

    let path = resolve_path(from, to)?;
    let mut state = WorkingState {
        position_m: position.as_array(),
        velocity_m_per_year: velocity.map(TerrestrialVelocityMPerYear::as_array),
    };
    for step in path.iter().take_while(|step| step.entry.is_some()) {
        let step = step.entry.expect("filtered step");
        state = apply_step(state, step, epoch_year)?;
    }

    Ok(TerrestrialState {
        position: TerrestrialPositionM::from_array(state.position_m)?,
        velocity: state
            .velocity_m_per_year
            .map(TerrestrialVelocityMPerYear::from_array)
            .transpose()?,
    })
}

const fn invalid_input(field: &'static str, reason: &'static str) -> FrameCatalogError {
    FrameCatalogError::InvalidInput { field, reason }
}

fn validate_epoch(epoch_year: f64) -> Result<(), FrameCatalogError> {
    validate_finite("epoch_year", epoch_year)
}

fn validate_finite(field: &'static str, value: f64) -> Result<(), FrameCatalogError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid_input(field, "must be finite"))
    }
}

fn validate_array3(fields: [&'static str; 3], values: [f64; 3]) -> Result<(), FrameCatalogError> {
    validate_finite(fields[0], values[0])?;
    validate_finite(fields[1], values[1])?;
    validate_finite(fields[2], values[2])
}

fn validate_helmert_parameters(parameters: &HelmertParameters) -> Result<(), FrameCatalogError> {
    validate_array3(
        [
            "translation_mm[0]",
            "translation_mm[1]",
            "translation_mm[2]",
        ],
        parameters.translation_mm,
    )?;
    validate_finite("scale_ppb", parameters.scale_ppb)?;
    validate_array3(
        ["rotation_mas[0]", "rotation_mas[1]", "rotation_mas[2]"],
        parameters.rotation_mas,
    )
}

fn validate_helmert_rates(rates: &HelmertRates) -> Result<(), FrameCatalogError> {
    validate_array3(
        [
            "translation_mm_per_year[0]",
            "translation_mm_per_year[1]",
            "translation_mm_per_year[2]",
        ],
        rates.translation_mm_per_year,
    )?;
    validate_finite("scale_ppb_per_year", rates.scale_ppb_per_year)?;
    validate_array3(
        [
            "rotation_mas_per_year[0]",
            "rotation_mas_per_year[1]",
            "rotation_mas_per_year[2]",
        ],
        rates.rotation_mas_per_year,
    )
}

#[derive(Debug, Clone, Copy)]
struct EvaluatedHelmert {
    translation_m: [f64; 3],
    scale: f64,
    rotation_rad: [f64; 3],
    translation_rate_m_per_year: [f64; 3],
    scale_rate_per_year: f64,
    rotation_rate_rad_per_year: [f64; 3],
}

impl EvaluatedHelmert {
    fn matrix(self) -> Mat3 {
        let [rx, ry, rz] = self.rotation_rad;
        [
            [1.0 + self.scale, -rz, ry],
            [rz, 1.0 + self.scale, -rx],
            [-ry, rx, 1.0 + self.scale],
        ]
    }

    fn rate_term(self, position_m: [f64; 3]) -> [f64; 3] {
        let [rx, ry, rz] = self.rotation_rate_rad_per_year;
        let scale = self.scale_rate_per_year;
        [
            self.translation_rate_m_per_year[0] + scale * position_m[0] - rz * position_m[1]
                + ry * position_m[2],
            self.translation_rate_m_per_year[1] + rz * position_m[0] + scale * position_m[1]
                - rx * position_m[2],
            self.translation_rate_m_per_year[2] - ry * position_m[0]
                + rx * position_m[1]
                + scale * position_m[2],
        ]
    }
}

#[derive(Debug, Clone, Copy)]
struct WorkingState {
    position_m: [f64; 3],
    velocity_m_per_year: Option<[f64; 3]>,
}

#[derive(Debug, Clone, Copy)]
struct PathStep {
    entry: Option<DirectedEntry>,
}

#[derive(Debug, Clone, Copy)]
struct DirectedEntry {
    transform: &'static HelmertTransform,
    reverse: bool,
}

fn apply_step(
    state: WorkingState,
    step: DirectedEntry,
    epoch_year: f64,
) -> Result<WorkingState, FrameCatalogError> {
    let evaluated = step.transform.evaluated(epoch_year)?;
    if step.reverse {
        apply_reverse(state, step.transform, evaluated, epoch_year)
    } else {
        Ok(apply_forward(state, evaluated))
    }
}

fn apply_forward(state: WorkingState, evaluated: EvaluatedHelmert) -> WorkingState {
    let matrix = evaluated.matrix();
    let position_m = add3(evaluated.translation_m, mul_vec3(&matrix, state.position_m));
    let velocity_m_per_year = state
        .velocity_m_per_year
        .map(|velocity| add3(velocity, evaluated.rate_term(state.position_m)));
    WorkingState {
        position_m,
        velocity_m_per_year,
    }
}

fn apply_reverse(
    state: WorkingState,
    transform: &HelmertTransform,
    evaluated: EvaluatedHelmert,
    epoch_year: f64,
) -> Result<WorkingState, FrameCatalogError> {
    let matrix = evaluated.matrix();
    let inverse = invert_3x3_adjugate(&matrix).ok_or(FrameCatalogError::SingularTransform {
        from: transform.from,
        to: transform.to,
        epoch_year,
    })?;
    let position_m = mul_vec3(&inverse, sub3(state.position_m, evaluated.translation_m));
    let velocity_m_per_year = state
        .velocity_m_per_year
        .map(|velocity| sub3(velocity, evaluated.rate_term(position_m)));
    Ok(WorkingState {
        position_m,
        velocity_m_per_year,
    })
}

fn resolve_path(
    from: TerrestrialFrame,
    to: TerrestrialFrame,
) -> Result<[PathStep; 3], FrameCatalogError> {
    let frames = [
        TerrestrialFrame::Itrf2020,
        TerrestrialFrame::Itrf2014,
        TerrestrialFrame::Itrf2008,
        TerrestrialFrame::Etrf2020,
    ];
    let mut queue = [TerrestrialFrame::Itrf2020; 4];
    let mut visited = [false; 4];
    let mut predecessor: [Option<(TerrestrialFrame, DirectedEntry)>; 4] = [None; 4];
    let mut head = 0_usize;
    let mut tail = 0_usize;

    visited[from.index()] = true;
    queue[tail] = from;
    tail += 1;

    while head < tail {
        let current = queue[head];
        head += 1;
        if current == to {
            break;
        }

        for neighbor in frames {
            if visited[neighbor.index()] {
                continue;
            }
            if let Some(step) = directed_entry(current, neighbor) {
                visited[neighbor.index()] = true;
                predecessor[neighbor.index()] = Some((current, step));
                queue[tail] = neighbor;
                tail += 1;
            }
        }
    }

    if !visited[to.index()] {
        return Err(FrameCatalogError::NoCatalogPath { from, to });
    }

    let mut reversed = [PathStep { entry: None }; 3];
    let mut count = 0_usize;
    let mut current = to;
    while current != from {
        let Some((previous, step)) = predecessor[current.index()] else {
            return Err(FrameCatalogError::NoCatalogPath { from, to });
        };
        if count == reversed.len() {
            return Err(FrameCatalogError::NoCatalogPath { from, to });
        }
        reversed[count] = PathStep { entry: Some(step) };
        count += 1;
        current = previous;
    }

    let mut path = [PathStep { entry: None }; 3];
    for i in 0..count {
        path[i] = reversed[count - 1 - i];
    }
    Ok(path)
}

fn directed_entry(from: TerrestrialFrame, to: TerrestrialFrame) -> Option<DirectedEntry> {
    TERRESTRIAL_FRAME_CATALOG.iter().find_map(|entry| {
        if entry.from == from && entry.to == to {
            Some(DirectedEntry {
                transform: entry,
                reverse: false,
            })
        } else if entry.from == to && entry.to == from {
            Some(DirectedEntry {
                transform: entry,
                reverse: true,
            })
        } else {
            None
        }
    })
}

# Changelog

All notable changes to `sidereon-core` are documented here.

## [0.19.0]

### Changed

- The fusion smoother's transition-combination step uses a specialized square
  matrix product, making fixed-interval smoothing tractable over histories
  recorded at inertial sample rates (found during the deep-urban field
  rematch).
- Correction to an earlier draft of this entry: a one-ULP evaluation shift on
  Earth-orientation-chain paths appeared mid-cycle from the tide-force wiring
  and was reversed by the station-displacement refactor before release. Net
  evaluation bits for these surfaces are UNCHANGED relative to 0.18.0.

### Added

- Station displacement corrections now have a public `tides` entry that accepts
  ITRF/ECEF or WGS84 geodetic station positions, UTC epochs, per-epoch IERS
  polar motion, and optional caller-supplied BLQ ocean-loading coefficients.
  The scalar and batch APIs return component-resolved ITRF/ECEF displacements
  for solid Earth tide, pole tide, and ocean tidal loading. BLQ parsing supports
  standard Bos-Scherneck/HARDISP six-row station blocks and reports typed errors
  for unsupported constituents.
- Solid Earth tide and solid Earth pole tide propagation forces. The solid
  Earth tide force ships the IERS 2010 Chapter 6 Step 1 frequency-independent
  anelastic Love-number `Cnm`/`Snm` corrections from Sun and Moon positions,
  including degree 3 terms and degree 4 `k+` terms from degree-2 tides. Step 2
  frequency-dependent constituent corrections are documented as a follow-up.
  The pole tide force uses polar-motion samples from a series-backed
  body-fixed provider and the IERS 2010 mean pole model, and both forces are
  opt-in builder components.

## [0.18.0]

### Added

- Loose GNSS updates can opt into IGG-III measurement variance inflation and a
  Yang two-segment prediction adaptive factor. The prediction factor is gated
  by a Jiang-Zhang Mahalanobis measurement-outlier check so measurement faults
  use measurement reweighting rather than innovation-driven covariance scaling.
- Fusion RTS fixed-interval smoothing over recorded error-state histories,
  with recorded forward-pass transitions, predicted and updated checkpoints,
  smoothed covariances, and loose/tight measurement-agnostic entry points.
- Simulator-backed field-behavior pins for loose fusion smoothing, outage
  coast, and low-satellite tight consistency.

## [0.17.0]

### Fixed

- Tight GNSS C1C and carrier-phase code-row prediction now uses the same
  measured-pseudorange transmit-time model as SPP, removing centimetre-level
  frozen-state residual differences from the prior observable transmit-time
  approximation.
- Sample-backed SP3 interpolation now reconstructs the whole-second node axis
  from the split epoch before reducing it to continuous J2000 seconds, with
  only an epoch-ULP bound for accepting whole-second candidates. An earlier
  attempt used an absolute snap that did not fire for real converted epochs,
  which land one `f64` ULP below affected record seconds; the record-epoch
  oracle now runs a conversion-path fixture on both construction paths.

### Changed

- IONEX slant-delay evaluation now reports out-of-coverage epochs and
  pierce-point latitude or longitude as typed errors by default instead of a
  silent hold. Callers can opt into the legacy hold behavior with
  `IonexCoveragePolicy::Hold`, which returns an explicit status marker, and the
  new batch result helper reports coverage per element.

## [0.16.1]

### Fixed

- SP3 interpolation on the parsed-product path now uses an exact parsed
  J2000-second epoch axis for record nodes, preventing one-second node bucketing
  errors at affected 45-minute cadence boundaries. Record-epoch positions and
  clocks are gated directly against public SP3 text records, including the
  cached batch path. (A clock quantization initially reported alongside this
  was traced to the reporting consumer's own time conversion, not to this
  library; the record-epoch clock oracle it prompted remains, at 5e-13 s
  against the file text.)

## [0.11.1]

### Added

- GNSS observation QC now computes teqc-style multipath (MP1/MP2 RMS) per
  satellite and per constellation with per-arc moving-average bias removal,
  matching teqc `+qc` to sub-micrometer on a real captured stream; a
  receiver clock-jump detector; and an aggregate per-constellation cycle-slip
  tally over the existing dual-frequency slip detector.
- QC report renderers: a fixed-width teqc-style text summary, an HTML summary,
  and JSON serialization of the full `ObservationQcReport`.
- RINEX 2.x observation-file ingest into the shared canonical observation IR,
  so RINEX 2 and CRINEX 1.0 archives parse and flow through QC and lint
  unchanged.
- Fuzz targets for the space-weather CSV/txt parser, the RINEX QC repair
  round-trip, and the EGM96 DTED grid parser.

### Changed

- Rust, Python, C, WASM, and Elixir interfaces expose the new QC surface
  (multipath, clock jumps, cycle-slip tally, and the report renderers)
  with uniform parity.

## [0.11.0]

### Added

- Core 6x6 orbit covariance transport with frame-labeled nodes,
  RTN acceleration process noise, caller-supplied transport segments,
  PSD-safe Log-Cholesky interpolation, covariance unit conversion helpers, and
  TCA Pc integration for propagated covariances.
- Space-weather ingestion: CSSI space-weather CSV and txt parsing with a
  time-indexed table, NRLMSISE-00 selection conventions (previous-day F10.7,
  81-day centered average, daily and 3-hourly Ap), format-faithful
  serializers, a CelesTrak data-catalog entry, and a `SpaceWeatherSource`
  hook feeding atmospheric drag and orbital-decay estimation.
- GNSS observation quality control: per-satellite and per-signal completeness,
  gap, and signal-strength summaries over RINEX observation data, plus a RINEX
  lint and repair pass with typed finding codes, cross-checked against an
  independent extraction oracle on real IGS stations. (Multipath, cycle-slip,
  and clock-jump metrics land in 0.11.1.)
- RTCM MSM carrier-phase lock-time indicator to RINEX loss-of-lock indicator
  derivation: DF402 and DF407 lock-time bucket tables, conservative
  decrease detection with same-bucket ambiguity handling, half-cycle ambiguity
  mapping, and a per-signal lock-time tracker, cross-checked against an
  independent RTKLIB convbin decode of a real MSM stream. Adds RTCM stream
  decode diagnostics and typed truncation classification.
- NTRIP sans-IO protocol: a caster handshake and streaming state machine
  (request builder, response classification, chunked and sourcetable decoding,
  GGA position feed policy) with no transport in the core, plus idiomatic
  streaming clients in the Python and Elixir interfaces.
- NMEA 0183 support: a forgiving sentence parser, an epoch accumulator, and a
  GGA writer over a format-agnostic representation.
- CNAV and RINEX-4 broadcast evaluation: CNAV and CNAV2 clock and orbit
  parameters, user range accuracy and inter-signal correction accessors,
  mixed broadcast-store selection, and a lenient navigation parse that reports
  skipped blocks.
- TLE mean-element fitting: fit SGP4 elements to a span of states on the
  shared trust-region least-squares engine, with observability diagnostics,
  epoch selection, and observation weighting. NDM epochs gain femtosecond
  precision through a single shared parser.
- Geoid undulation evaluation matching PROJ on the EGM96 15-arcminute grid:
  node-registered bilinear interpolation with antimeridian and pole handling,
  batch lookup, and orthometric to ellipsoidal height conversion, pinned to
  PROJ-computed reference values.

### Changed

- `Covariance6Error` now includes interpolation-specific
  `NotFactorizable` and `InvalidInterpolationParameter` variants, and
  propagated-covariance TCA option structs now carry `process_noise`.
  Exhaustive matches and struct literals may need source updates.
- Rust, Python, C, WASM, and Elixir interfaces expose uniform capability parity
  for the 0.11.0 surface.

## [0.10.1]

### Fixed

- DTED ten-degree block directories now follow the layout production stores
  use: the hemisphere letter comes from the tile index and the magnitude is the
  truncated absolute value, so `n36_w107` buckets under `n30_w100/`,
  `n32_w118` under `n30_w110/`, and `s01_w001` under `s00_w000/` (with `n00`
  and `s00` kept distinct). The previous flooring convention mis-bucketed
  every western and southern index that was not an exact multiple of ten,
  making caches invisible to existing tile stores. Tile naming itself is
  unchanged. A cache directory populated by 0.10.0 can be migrated by moving
  the affected tiles into the corrected block directories, or simply
  regenerated. The derivation is validated against an observed 888-tile
  listing captured from a production-style store.
- `PreciseEphemerisSamples::from_samples` now rejects a sample epoch whose
  derived J2000 seconds is not finite and a finite clock offset that overflows
  to a non-finite value in native microseconds, instead of poisoning the
  interpolation node axis or emitting non-finite clock values downstream.

## [0.10.0]

### Added

- Astrodynamics coverage for anomaly conversions, analytic Kepler propagation,
  equinoctial and modified-equinoctial elements, solar beta angle,
  RIC/RTN/LVLH relative frames, Clohessy-Wiltshire motion, angular separation,
  position angle, general body observation, almanac events, atmospheric drag
  force, orbital decay, source-agnostic ephemeris grid sampling, and
  terrain/DTED lookup.
- GNSS DCB/OSB bias ingestion, SBAS augmentation with decode and corrected SPP,
  SSR/HAS real-time corrections, and robust SPP with a fault
  detection/exclusion driver.
- Cache-first data acquisition support for SP3, IONEX, CLK, NAV, and SRTM
  terrain to DTED products, using a single sans-IO core catalog and bit-exact
  hgt to DTED conversion.

### Changed

- Rust, Python, C, WASM, and Elixir interfaces now expose uniform capability
  parity for the 0.10.0 surface.
- GNSS constellation labels now use conventional styling: GPS, GLONASS, Galileo,
  BeiDou, QZSS, NavIC, and SBAS.

## [0.1.0]

Initial release.

- SGP4/SDP4 propagation (Vallado port), TLE and OMM (KVN/XML/JSON) parsing.
- Coordinate and time transforms (TEME/GCRS/ITRS/geodetic/topocentric, leap
  seconds, UT1), Sun/Moon ephemeris, solid-earth tides.
- RINEX navigation/observation/clock and CRINEX parsing, SP3 load and merge,
  ANTEX antenna corrections, broadcast and precise ephemeris evaluation.
- GNSS positioning: SPP (with robust estimation), RTK (LAMBDA ambiguity
  resolution, dual-frequency, multi-GNSS), and static PPP.
- Carrier-phase combinations and cycle-slip detection, DOP, visibility and
  pass prediction, velocity/Doppler, and observation quality weighting.
- Conjunction assessment and collision probability.

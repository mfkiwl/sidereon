# sidereon

[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.21174997.svg)](https://doi.org/10.5281/zenodo.21174997)

GNSS positioning and astrodynamics in Rust, with first-class interfaces in Python, C, WebAssembly, and Elixir. Reference-validated, and bit-exact to public oracles where it counts.

sidereon is one engine: a Rust core for satellite orbit propagation, GNSS positioning, time and frame transforms, atmosphere models, and the standard exchange formats, exposed through idiomatic interfaces in five languages so the same validated math is reachable wherever you work.

**Live demo: [sidereon.dev](https://sidereon.dev)**: a real-time satellite tracker (globe, ground tracks, coverage, conjunction screening, orbit determination) computed in the browser via the WebAssembly build.

## Capabilities

- **Orbit propagation:** SGP4/SDP4 from TLE/OMM, numerical propagation with atmospheric drag and decay/reentry prediction, Kepler propagation and anomaly conversion, batch/constellation propagation, ground tracks, passes, visibility, and coverage.
- **GNSS positioning:** single-point (SPP), RTK float and fixed (LAMBDA), PPP float and fixed, DGNSS, across GPS/GLONASS/Galileo/BeiDou/QZSS, with robust solves, RAIM fault detection and exclusion, and DOP (G/P/H/V/T).
- **GNSS corrections:** SBAS message decode and correction application, RTCM SSR and Galileo HAS orbit/clock/bias correction stores, and differential code biases (DCB/OSB) from Bias-SINEX and CODE products.
- **Ephemeris and time:** broadcast and precise (SP3) ephemeris, JPL SPK kernels, source-agnostic satellite state sampling across all three, scale-aware time (UTC/TT/UT1/TDB/GNSS), leap seconds, Earth orientation (EOP).
- **Geometry and events:** TEME/GCRS/ITRS/geodetic/topocentric transforms (IAU/IERS), look angles, eclipse, relative motion in RIC/RTN/LVLH frames with Clohessy-Wiltshire propagation, conjunction screening with collision probability (TCA/Pc), initial orbit determination, and classical and equinoctial element conversion.
- **Observation and almanac:** apparent places (geocentric and topocentric RA/Dec) for the Sun, Moon, and planets, Sun and Moon rise/set, seasons, moon phases, planetary transits, lunar and solar eclipses, sub-solar/sub-observer and terminator geometry, angular separation and position/phase/beta angles, and satellite visual magnitude.
- **Atmosphere and terrain:** Klobuchar and full Galileo NeQuick-G ionosphere, IONEX grids (vertical TEC and slant delay), tropospheric delay, NRLMSISE-00 density, and DTED terrain elevation lookup.
- **RF link:** free-space path loss, EIRP, carrier-to-noise (C/N0), and link margin.
- **Formats:** TLE/OMM, CCSDS OEM/OPM/CDM, RINEX observation/navigation/clock, CRINEX (Hatanaka), SP3, IONEX, ANTEX, Bias-SINEX, CODE DCB, RTCM 3.x, with forgiving parsers and round-trippable serializers for the formats that support it.

## Install

```sh
cargo add sidereon
```

```rust
use sidereon::astro::passes::{look_angle, GroundStation, UtcInstant};

let line1 = "1 25544U 98067A   24001.50000000  .00016717  00000-0  10270-3 0  9009";
let line2 = "2 25544  51.6400 208.8657 0002644 250.3037 109.7782 15.49560812999990";

let elements = sidereon::astro::tle::parse(line1, line2)?.elements.to_element_set()?;
let station = GroundStation { latitude_deg: 51.5, longitude_deg: -0.1, altitude_m: 10.0 };
let when = UtcInstant::from_utc(2024, 1, 1, 12, 0, 0, 0).ok_or("bad datetime")?;

let look = look_angle(&elements, station, when)?;
println!("az {:.2} el {:.2} range {:.1} km", look.azimuth_deg, look.elevation_deg, look.range_km);
```

More runnable examples, in all five languages, are on the [live demo](https://sidereon.dev).

## Crates

- [`sidereon-core`](crates/sidereon-core): the engine. SGP4/SDP4 propagation, coordinate and time transforms, RINEX/SP3/ANTEX/OMM/RTCM parsing, broadcast and precise ephemeris, SPP/RTK/PPP/DGNSS positioning, DOP and visibility, conjunction assessment, and the supporting numerical kernels.
- [`sidereon`](crates/sidereon): the ergonomic Rust interface over `sidereon-core`. Product loaders plus SPP/RTK/PPP solves with result structs and one error enum. This is the Rust interface, held to the same parity bar as the bindings below.
- [`trust-region-least-squares`](crates/trust-region-least-squares): a standalone, independently publishable nonlinear least-squares solver that reproduces SciPy's trust-region-reflective `least_squares` bit-for-bit. It does not depend on the engine.

## Interfaces

The Rust interface is the `sidereon` crate above. The other language interfaces live in their own repositories, each over the same core:

- Python: [`sidereon-python`](https://github.com/neilberkman/sidereon-python) (PyPI: `sidereon`)
- C: [`sidereon-c`](https://github.com/neilberkman/sidereon-c)
- Elixir: [`sidereon-ex`](https://github.com/neilberkman/sidereon-ex) (Hex: `sidereon`)
- JavaScript / WebAssembly: [`sidereon-wasm`](https://github.com/neilberkman/sidereon-wasm) (npm: `@neilberkman/sidereon`)

## Validation

Every numerical routine is cross-checked against the reference implementation or published standard for its domain, not just internal goldens. Each check runs in the test suite against a committed reference fixture whose provenance (tool, version, source) is recorded, and many gates are bit-for-bit.

| Capability | Cross-checked against | Reference |
|---|---|---|
| SGP4 / SDP4 propagation | Vallado / CelesTrak verification states | Vallado, Crawford, Hujsak & Kelso, *Revisiting Spacetrack Report #3*, AIAA 2006-6753 |
| Frames, time, ephemerides (TEME/GCRS/ITRS, IAU 2006/2000A) | Pinned Skyfield vectors (ERFA/SOFA routines under the hood) | IAU SOFA / ERFA |
| Earth orientation | IERS Earth-orientation parameters | Petit & Luzum (eds.), *IERS Conventions (2010)*, IERS TN 36 |
| Least-squares engine (TRF) | SciPy `least_squares`, bit-exact | Virtanen et al., *SciPy 1.0*, Nature Methods 17 (2020) |
| GNSS positioning (SPP / RTK / PPP) | RTKLIB oracle arcs and real IGS precise products | RTKLIB; International GNSS Service (IGS) |
| Geoid undulation (EGM96 15-arcminute) | PROJ (`us_nga_egm96_15`), to 5 mm | PROJ |
| RINEX observation QC — multipath (MP1/MP2) | teqc `+qc` on a real captured stream, to sub-micrometer | teqc |
| RTCM MSM lock-time to RINEX LLI | RTKLIB `convbin` decode of a real MSM stream | RTKLIB |

The per-crate test suites document the exact references, fixtures, and tolerances.

## License

MIT

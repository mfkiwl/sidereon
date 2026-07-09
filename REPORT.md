# Lane PF Report

## Facade Root

| Audit item | Status | Proof |
|---|---|---|
| Row #39: CRINEX encode | Closed. Added root `sidereon::encode_crinex` as a thin wrapper over the existing lower core encoder with facade error mapping. | `product_ingestion_wrappers_parse_fixture_products` encodes decoded real CRINEX fixture text, decodes the emitted CRINEX, and asserts the RINEX text round-trips. |
| Reverse gap: wasm CRINEX encode | Closed for this lane when the parity checker is run with `--rust-root /private/tmp/pl-core`; detected Rust root alias `encode_crinex` closes `wasm_crinex_encode`. | `python3 parity_check.py --workspace /Users/neil/xuku --rust-root /private/tmp/pl-core` reports it under `Reverse gaps closed by detected Rust facade aliases`. |
| Reverse gap: wasm moon/sky convenience functions | Closed where lower Rust already existed. Added root re-exports for `sun_az_el`, `moon_az_el`, `moon_illumination`, `find_moon_elevation_crossings`, and `find_moon_transits`. | `ground_site_sun_moon_helpers_reachable_through_facade` exercises the root names with real Greenwich Sun/Moon dates and event counts. |
| Reverse gap: Elixir root geodetic/look-angle/doppler conveniences | Closed where lower Rust already existed. Added root re-exports for geodetic/topocentric transforms, TLE look-angle/ground-track helpers, and Doppler helpers. | `root_geodetic_look_angle_and_doppler_helpers_reachable_through_facade` exercises root names through real TLE parsing, geodetic conversions, ground track, and Doppler values. |

No new math was implemented. Python data/NTRIP workflow helpers, C ABI/version/lifecycle functions, C accessor families, wasm package internals, Elixir raw NIF stubs, and similar language-runtime surfaces remain intentionally not mirrored as facade root math APIs.

## Parity Checker

| Work item | Status | Proof |
|---|---|---|
| Per-interface root overrides | Closed. Added `--rust-root`, `--python-root`, `--wasm-root`, `--c-root`, `--elixir-root`, plus `SIDEREON_PARITY_<IFACE>_ROOT`. | Default and `--rust-root /private/tmp/pl-core` runs use different Rust symbol counts and roots. |
| Live scan authoritative | Closed. Alias-bearing cells now derive `yes`/`no` from the scan; static status remains only for cells with no aliases and is listed in the report. | Default run changes `raim_standalone` Python `partial->yes`, wasm `no->yes`, and Elixir `partial->yes`; RAIM partial was stale baseline, not a detectable difference. |
| Reverse allowlist | Closed. Added `reverse_gap_allowlist.json` with rationale entries for language-specific reverse gaps. | Default run fails on 5 unallowed reverse gaps instead of all 17; lane Rust-root run fails on 2 unallowed reverse gaps after facade closures. |
| Audit count reconciliation | Partially closed with documentation. The reverse count still reproduces 17 under old defaults. Forward rows now report 34 instead of 30 because live alias scans supersede static partial/yes cells; every transition is printed under `SCAN-derived status changes`. | `python3 parity_check.py --workspace /Users/neil/xuku` output documents the exact transitions. |

## Final Gap Table Against 0.24.0 Mains

Command:

```sh
python3 /Users/neil/xuku/sidereon_docs/tools/parity_check/parity_check.py --workspace /Users/neil/xuku
```

Result summary: 34 forward gap rows, 17 reverse gaps, 50 unallowed forward cells, 5 unallowed reverse gaps. The top ranked gap changed from `raim_standalone` to `rinex_obs_spp_convenience` because direct RAIM is live-detected in Python, wasm, C, and Elixir.

Forward gap rows:

| Capability | Cells |
|---|---|
| `rinex_obs_spp_convenience` | python:no, wasm:no, c:no, elixir:no |
| `raim_for_solution` | python:no, wasm:no |
| `broadcast_fde` | python:no, wasm:no |
| `araim_lpv200_fault_modes` | elixir:no |
| `wide_lane_rtk` | elixir:no |
| `rinex_nav_parse_load` | elixir:no |
| `rinex_obs_parse_load` | elixir:no |
| `fusion_inertial_surface` | c:no, elixir:no |
| `terrain_geoid_data_surface` | elixir:no |
| `spp_solve_precise_ephemeris` | elixir:no |
| `spp_batch_solve` | elixir:no |
| `doppler_assisted_spp_velocity_solve` | elixir:no |
| `rinex_static_rtk_baseline_solve` | elixir:no |
| `standalone_range_raim_fde_design` | rust:no |
| `sp3_load_interpolate` | elixir:no |
| `sp3_merge_frame_reconciliation` | rust:no |
| `precise_samples_interpolant` | elixir:no |
| `precise_interpolant_artifact_mmap` | rust:no, elixir:no |
| `decode_stream_frame_message` | c:no |
| `encode_message_frame` | python:no, c:no |
| `ssr_decode_store_corrected_state` | python:no, c:no, elixir:no |
| `sbas_decode_corrected_state` | c:no |
| `sbas_protection_levels` | elixir:no |
| `parse_group_epochs_write_gga` | c:no, elixir:no |
| `ntrip_core` | rust:no, elixir:no |
| `geometry_quality_labels` | elixir:no |
| `ephemeris_sample_rows` | rust:no, elixir:no |
| `staleness_selection_fallback` | elixir:no |
| `parse_encode_parse_file_checksum_warnings` | python:no, wasm:no, c:no |
| `propagate_batch_decay_latch` | c:no |
| `fspl_eirp_c_n0_link_budget` | rust:no, c:no |
| `trls_solve_drop_one_report` | c:no |
| `allan_normality_tests_covariance_derived_error_metrics` | elixir:no |
| `signal_analysis_surface` | c:no |

Unallowed reverse gaps against 0.24.0 mains:

| Reverse gap | Interface |
|---|---|
| `wasm_crinex_encode` | wasm |
| `c_raim_for_solution` | c |
| `elixir_root_geodetic_look_angle_doppler_conveniences` | elixir |
| `wasm_moon_sky_convenience_functions` | wasm |
| `c_robust_fde_broadcast_sp3_split` | c |

With this lane's Rust root override, `wasm_crinex_encode`, `elixir_root_geodetic_look_angle_doppler_conveniences`, and `wasm_moon_sky_convenience_functions` are closed by detected Rust facade aliases; 14 reverse gaps remain, with 2 unallowed reverse gaps: `c_raim_for_solution` and `c_robust_fde_broadcast_sp3_split`.

## Gates

| Command | Exit |
|---|---:|
| `cargo test --workspace` | 0 |
| `cargo clippy --all-targets -- -D warnings` | 0 |
| `cargo fmt --check` | 0 |
| `(cd fuzz && cargo check)` | 0 |
| `python3 -m py_compile /Users/neil/xuku/sidereon_docs/tools/parity_check/parity_check.py` | 0 |

The parity checker itself exits `1` against both default mains and this lane root because remaining unallowed gaps are still actionable; this is expected and recorded in the tables above.

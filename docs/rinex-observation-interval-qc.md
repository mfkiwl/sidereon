# RINEX observation `INTERVAL` QC evidence

This note records the public evidence and Sidereon policy used for observation
interval parsing, quality control, and repair in release 0.35.0. Sources were
accessed on 2026-07-24.

## Public evidence

| Source | Relevant public requirement |
| --- | --- |
| [RINEX 2.11](https://files.igs.org/pub/data/format/rinex211.txt), section 5.3 and Table A2 | An item unknown at file creation may be zero, blank, or omitted. `INTERVAL` is the optional observation interval in seconds. |
| [RINEX 3.05](https://files.igs.org/pub/data/format/rinex305.pdf), section 6.5 and Table A2 | An unknown item may be zero, blank (`BNK`), or omitted. `INTERVAL` is optional and uses an `F10.3` seconds field. Readers should report unknown records or types to users. |
| [RINEX 4.02](https://files.igs.org/pub/data/format/rinex_4.02.pdf), section 6.5 and Table A2 | An unknown header item may be zero, blank (`BNK`), or omitted. `INTERVAL` remains an optional `F10.3` seconds field. |
| [Scheduled fuzz run 30086311906](https://github.com/neilberkman/sidereon/actions/runs/30086311906) | The `rinex_qc_repair_roundtrip` target found that a successfully parsed zero interval reached an infallible QC wrapper that assumed every present interval was usable and panicked on `InvalidInterval`. |

The specifications establish that zero is valid syntax for unavailable
metadata. They do not establish zero as a usable observation cadence. A
negative interval has no standards-defined unavailable meaning, and a
non-finite value cannot be represented by the specified fixed-point field.

## Public library contract

- A finite, positive source interval may be used as the header cadence.
- A zero source interval is retained as unavailable metadata and reported as
  informational `OBS-H19`.
- A negative source interval, or a non-finite value in a caller-constructed
  product, is reported as error `OBS-H20`.
- Zero, negative, and non-finite source values are never used for gap
  calculations. QC either infers a positive cadence from the parsed epoch grid
  and labels its source `Inferred`, or labels it `Unresolved` and performs no
  interval-dependent gap calculation.
- An explicit caller override must be finite and positive. Invalid overrides
  return `InvalidInterval`.
- Source interval repair is opt-in. When requested, repair replaces an
  unusable value with an inferred cadence or removes the optional field when no
  cadence can be inferred.
- Blank `INTERVAL` fields parse as absent metadata. Malformed nonblank numeric
  fields remain parse errors.

This behavior accepts standards-compatible unknown metadata without trusting
it, inventing a cadence, or hiding the condition from the report.

## Deterministic regressions

The exact 583-byte Actions artifact is retained losslessly as hexadecimal in
`crates/sidereon-core/tests/fixtures/qc/` and decoded by a core regression test:

```text
SHA-256 8091e235ba62ac613623e4d3c1856e8d0b9e04685584e42bed993fe8f9c2838a
```

Its 304-byte human-readable reduction is committed both as a core fixture and
as the `rinex_qc_repair_roundtrip` corpus seed:

```text
SHA-256 5fb8bee02018479d441792d13aa9acaf45a27e03ddaaf67773a376809ba6d748
```

Tests exercise parsing, default QC, lint visibility, opt-in repair,
repair/reparse idempotence, the fuzz target, and the user-facing CLI JSON path.
Additional adversarial tests cover negative and non-finite values, extremely
small positive caller intervals, saturating missing-epoch counts, and
sub-millisecond inference that would otherwise round to zero.

The correction changes parser and QC reporting behavior only. Solver, orbit,
propagation, positioning, frame, timing, and other numerical kernels are
unchanged.

# Third-Party Notices

`sidereon-core` is licensed under the MIT License, reproduced in `LICENSE`.

The crate contains or derives code and data from these permissively licensed
public sources:

- IERS Conventions software: the derived-work description and renamed Rust
  routines are in `src/tides/`; the intact terms are in
  `IERS-CONVENTIONS-SOFTWARE-LICENSE.txt` (official source accessed
  2026-07-20: https://iers-conventions.obspm.fr/content/chapter7/software/dehanttideinel/DEHANTTIDEINEL.F).
  This Sidereon derived work is neither distributed by nor endorsed by the IERS
  Conventions Center.
- ERFA/SOFA: precession/nutation conventions and coefficient tables; the
  applicable terms are in `ERFA-LICENSE.txt`.
- RTKLIB: the MLAMBDA/LAMBDA integer least-squares port; the applicable terms
  are in `RTKLIB-LICENSE.txt`.
- SciPy: trust-region least-squares routines arrive through the separately
  packaged `trust-region-least-squares` dependency, which carries the SciPy
  license and derivation notice.

No copyleft code is included.

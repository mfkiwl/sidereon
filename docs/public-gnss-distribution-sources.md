# Public GNSS distribution sources

## Decision

Sidereon models an exact GNSS product independently from the distributor used
to obtain it. A distributor may change the public URL and transport compression;
it cannot change the publisher, product line, solution class, issue, date,
cadence, family, format, or official decompressed filename.

The Rust core remains network-free. It owns catalog selection, exact identity,
official filename and source-location derivation, safe cache-relative paths,
and SP3/IONEX parsing. Bindings that already own acquisition—Python and
Elixir—own authenticated HTTP, retries, cookies, cache IO, and credential
configuration. C and WebAssembly expose the same pure identity and location
derivation without adding hidden network behavior.

## Public model

`ProductIdentity` contains the public product family, publisher, solution class,
campaign token, filename version, date, issue/start time, coverage token,
sampling token, official filename, format, and prediction horizon when the
catalog line defines one. `ProductIdentity::validate` rejects a caller-built
value when those fields disagree with the filename. URL, request, and cache-key
helpers all invoke that validation before using the value.

`DistributionSource` has four explicit values:

- `direct`: the existing cataloged analysis-center or IGS archive;
- `nasa_cddis`: NASA CDDIS over HTTPS;
- `local_file`: bytes read from a caller-selected path;
- `in_memory`: bytes supplied by the caller.

`DistributionLocation` records the source, public URL when one exists, archive
filename, and transport compression. The official filename in the identity is
the decompressed standard-product filename. That permits two distributors to
serve the same exact bytes with different transport packaging without treating
compression as a different scientific product.

`ProductRequest` holds one validated identity and a non-empty ordered list of
acceptable distributors. It grants no permission to try another analysis
center, tier, issue, date, cadence, or family.

## Complete exact product sets

Workflows that require several products can declare the full identity inventory
and call `validate_exact_product_set(expected, available)` after every product
has passed acquisition validation. The gate rejects an empty declaration,
duplicates on either side, missing identities, and undeclared identities. It
compares complete identities rather than filenames, so two prediction tiers
that publish the same filename remain distinct.

The gate is sans-IO and does not make cache writes transactional. Its contract
is that dependent processing starts only after it returns `Ok(())`. Pass only
resolved identities from successful acquisitions as `available`.

SP3 observed/predicted timing is a separate content property. Read it from
`Sp3::prediction_summary()`, which aggregates the product's record flags. Do
not derive that boundary from issue times, nominal durations, or catalog
prediction fields.

## CDDIS paths

IGS combined final SP3 identity is date-aware. The official rapid/final orbit
combination begins at GPS week 0730 (1994-01-02), so earlier final-SP3 requests
are rejected. Before GPS week 2238 the official decompressed filename is
`igs<gps-week><day>.sp3`; from week 2238 it is
`IGS0OPSFIN_<YYYY><DDD>0000_01D_15M_ORB.SP3`. CDDIS uses a four-digit GPS-week
directory and preserves the matching transport compression in each era:

```text
https://cddis.nasa.gov/archive/gnss/products/<four-digit-gps-week>/igs<gps-week><day>.sp3.Z
https://cddis.nasa.gov/archive/gnss/products/<four-digit-gps-week>/<official-filename>.gz
```

The cutoff comes from the IGS transition guideline, and archive objects on
both sides of the boundary confirm it. Current IGS final SP3 is classified as
`final`; IGS broadcast navigation remains `broadcast`. Call
`product_solution_class(center, family)` when the family is known. The legacy
center-only `AnalysisCenter::solution_class()` remains for source
compatibility, but cannot express both IGS product lines.

The BKG archive supports the current
`IGS/products/<gps-week>/<long-filename>.gz` layout. Its historical listings do
not establish one uniform direct path: week 2235 legacy products are under
`IGS/products/orbits/2235`, while week 2236 contains long-name trial products
under `IGS/products/2236` and only a partial legacy set under
`IGS/products/orbits/2236`. Sidereon therefore returns
`UnsupportedDistributionEra` for a pre-week-2238 IGS final-SP3 direct-BKG
location instead of guessing. The same exact historical identity can be
resolved through the verified CDDIS layout.

Current long-name IONEX products resolve to:

```text
https://cddis.nasa.gov/archive/gnss/products/ionex/<year>/<day-of-year>/<official-filename>.gz
```

The core rejects CDDIS requests for product families for which this mapping is
not implemented. It does not relabel another file as the requested product.

## CODE product routes

AIUB publishes product families in distinct directories behind its HTTPS
download service. Sidereon routes each catalog family independently:

```text
CODE MGEX final SP3/clock:  https://www.aiub.unibe.ch/download/CODE_MGEX/CODE/<year>/...
CODE final IONEX:          https://www.aiub.unibe.ch/download/CODE/<year>/...
CODE rapid IONEX:          https://www.aiub.unibe.ch/download/CODE/...
CODE ultra-rapid SP3:      https://www.aiub.unibe.ch/download/CODE/...
```

The `cod` SP3 and clock catalog entries describe the current MGEX final line;
its IONEX entry describes the operational final line. Historical CODE
short-name products use different identities and layouts. Until those are
modeled explicitly, `AnalysisCenter::Cod` rejects SP3, clock, and IONEX dates
before GPS week 2238 with `UnsupportedProductEra`; it never fabricates a
current long filename for a historical request.

CODE P1 and P2 predicted maps use separate AIUB tiers. Direct locations resolve
the exact product identity to:

```text
https://www.aiub.unibe.ch/download/CODE/IONO/P1/<identity-year>/<official-filename>.gz
https://www.aiub.unibe.ch/download/CODE/IONO/P2/<identity-year>/<official-filename>.gz
```

The HTTPS redirect chain is restricted to AIUB's download host and public
object-store host. A missing exact URL remains a not-published result; direct
location derivation performs no date lookback or tier substitution.

## GFZ rapid SP3 cadence eras

GFZ changed its operational rapid-orbit cadence inside GPS week 2158. Its
official listing publishes `GFZ0OPSRAP_20211370000_01D_15M_ORB.SP3.gz` for
2021 day 137 and `GFZ0OPSRAP_20211380000_01D_05M_ORB.SP3.gz` for day 138; the
subsequent products in that directory retain `05M`. Current rapid listings
also publish `05M`, including the verified 2026 day-200 object.

`default_sample(AnalysisCenter::Gfz, ProductType::Sp3)` retains its date-free
signature and now reports the current `05M` convention. Code deriving a dated
product should use `default_sample_for_date`, which returns `15M` through 2021
day 137 and `05M` from day 138. All catalog helpers that receive `sample=None`,
including `product` and `mgex_sp3`, use the date-aware query. An explicit
sampling token remains explicit and is not silently rewritten.

```text
through 2021-05-17: https://isdc-data.gfz.de/gnss/products/rapid/w2158/GFZ0OPSRAP_20211370000_01D_15M_ORB.SP3.gz
from 2021-05-18:    https://isdc-data.gfz.de/gnss/products/rapid/w2158/GFZ0OPSRAP_20211380000_01D_05M_ORB.SP3.gz
current example:    https://isdc-data.gfz.de/gnss/products/rapid/w2428/GFZ0OPSRAP_20262000000_01D_05M_ORB.SP3.gz
```

## Exact SP3 acceptance

`Sp3::parse` remains the general SP3 reader. Exact-product acquisition uses
`ExactSp3Request` with `parse_exact_sp3` or `validate_exact_sp3` before it
accepts bytes for a declared identity. That gate:

- accepts only a positive, fixed-duration requested cadence and rejects
  unknown units, `00U`, noncanonical equivalents such as `60M` or `24H`, and
  non-finite, zero, negative, or out-of-range header cadence;
- requires the header cadence to equal the requested sample interval;
- requires the line-1 start, line-2 GPS-week/seconds-of-week/MJD start fields,
  and first parsed epoch to represent the requested start, and requires the
  header epoch count to equal the parsed count;
- requires the mandatory SP3 header/EOF structure, exact agreement between the
  line-3 satellite declaration and per-epoch P/V record count and order,
  P-then-V pairing for velocity products, at least four header comment records,
  and no nonblank records after `EOF`;
- optionally binds the SP3 line-1 producing-agency field to the exact catalog
  identity (including the official `ESOC` and `AIUB` header codes, which differ
  from the `ESA` and `COD` filename producer codes);
- requires a strictly increasing, regular parsed epoch grid at the requested
  cadence; and
- derives the permitted count from the validated request span and cadence. A
  one-day, five-minute product may contain 288 half-open epochs ending at
  23:55, or 289 inclusive epochs ending at the next midnight. Shorter, longer,
  or irregular grids fail integrity validation.

The requested duration, rather than an untrusted SP3 header value, is the
source of truth for the coverage check. The SP3-d header still supplies an
independent cadence and count that must agree.

## Candidate fallback

Candidate resolution distinguishes ordinary publication absence from product
integrity failure. An explicitly recognized not-posted response can advance to
the next officially cataloged candidate. Malformed or unparseable bytes,
digest failure, start/identity mismatch, cadence mismatch, irregularity, and
span mismatch are terminal by default, and the first such error is preserved.
Caller configuration errors and unsupported center/product pairs are also not
reported as publication absence.

No official material reviewed for this audit documented a moving-alias rule
that permits broad fallback after failed content validation, so Sidereon adds
no such exception. A future exception would require a documented archive race
and a narrow dedicated state; it must not catch the general integrity-error
class.

## Authentication and transport boundary

Credentials are binding inputs, never core state. Python and Elixir accept a
caller-supplied Earthdata bearer token or the documented netrc mechanism.
Authentication headers are restricted to the approved CDDIS and Earthdata
Login hosts. Redirects are explicit and HTTPS-only for that flow. Cookies obey
their host/domain, path, and secure restrictions. Recorded URLs omit user
information, queries, and fragments, and neither errors nor provenance contain
headers, cookies, tokens, or passwords.

Acquisition distinguishes authentication required, authentication failed,
authorization denied, absent/not-yet-published, retired endpoint, redirect
policy, malformed URL, transport, content type, obvious HTML error document,
content length, decompression, caller checksum, product validation, and cache
failures. Retries are bounded and limited to connection/timeouts, HTTP 408/429,
and server errors.

## Content validation and provenance

A successful network status is not sufficient. Acquisition applies archive and
decompressed size limits, checks declared content length, rejects HTML, verifies
gzip completion and caller checksums, parses the standard product, and checks
its start date/time and cadence against the exact request. The resolved identity
adds the observed SP3 or IONEX format version.

Success returns the verified local path plus provenance containing requested
and resolved identity, publisher, distributor, official filename, sanitized
original and final URLs, retrieval time, decompressed and archive byte lengths
and SHA-256 hashes, compression, ETag/Last-Modified when available, cache-hit
state, and sanitized failures from earlier explicitly allowed distributors.

## Cache policy

The cache separates distributor and every exact identity discriminator. The
decompressed product, original downloaded archive, and JSON provenance sidecar
are retained. A cache hit rechecks identities, byte counts, both hashes, caller
checksum, and a fresh product parse with semantic checks.

## Merged-SP3 input identity

Every contributor accepted by a merged-SP3 acquisition has two separate public
records. `Sp3ArtifactIdentity` is reproducible: it binds requested and resolved
`ProductIdentity`, the selected distributor, official decompressed filename,
SHA-256 digest and byte length of both product and distributor archive, and
archive compression. Retrieval time, cache-hit status, sanitized URLs, HTTP
metadata, and failed attempts are acquisition observations and do not enter the
artifact identity.

`Sp3MergeInputIdentity::new` validates complete artifact records and binds the
canonical contributor set plus every `MergeOptions` control to a versioned
`sidereon-sp3-merge-input-v1:<sha256>` identifier. Contributor enumeration and
set/map iteration order do not affect that identifier for mean or median
combination. With precedence combination, contributor order is an effective
merge-policy control and is therefore bound in order; reversing it can change
the merged bytes and changes the identifier. A different verified artifact,
resolved identity, contributor set, or merge option also changes it. Empty,
duplicate, malformed, non-SP3, or internally mismatched contributor records are
rejected rather than inferred from filenames or cache contents.

The stable identifier deliberately contains no retrieval observations,
credentials, cookies, headers, URLs, or filesystem paths. Persist the public
artifact records and merge policy alongside it; `verify` recomputes the
canonical identifier from those records. Single-contributor and
multi-contributor merges use the same schema.

Accepted negative-zero tolerances canonicalize to positive zero because both
values execute identically. The literal public contract vectors in
[`sp3-merge-input-v1.json`](../crates/sidereon-core/golden/sp3-merge-input-v1.json) bind
the complete policy and exact artifacts for Rust, Python, Elixir, C, and WASM.

The acquisition-capable Python and Elixir interfaces publish the product,
original archive, and JSON provenance as one immutable transaction. A single
SHA-256-bound commit record names that transaction and is atomically replaced
only after the entry files and directories have been synchronized. Readers
follow only that record and then repeat the identity, source, digest, length,
caller-checksum, and semantic checks.

On Linux and macOS, both interfaces use the same per-entry POSIX advisory lock
across cache validation, acquisition, and commit. The wait is bounded; a lock or
cache-write failure is terminal rather than permission to try another source.
OS process death releases the lock automatically, allowing a later owner to
clean abandoned transactions without deleting a live writer's work. Valid
0.29.0-0.29.2 three-file entries are revalidated and migrated into the committed
layout without a new download.

The crash guarantee relies on a local filesystem providing atomic
same-directory rename, POSIX advisory locks, regular-file synchronization, and
directory synchronization. Under those Linux/macOS guarantees, a process death
or power loss during publication leaves the previous complete entry or no
acceptable entry; it cannot expose a mixed payload/provenance pair. A verified
existing entry is returned without contacting a remote service, including in
offline mode.

The [cache atomicity audit](exact-product-cache-atomicity.md) records the 0.29.2
verdict, corrected protocol, process/failpoint coverage, compatibility, and
residual risks.

## Compatibility and extension

Python and Elixir route the legacy IONEX convenience API through exact
acquisition. Its explicit lookback option still controls candidate dates, but
each candidate now uses the versioned exact cache and full semantic validation;
unverified entries in the former flat cache are not accepted implicitly.
Adding another public distributor requires a location/compression mapping for
an existing identity plus the same redirect, size, content, parse, provenance,
and cache gates. It must not modify identity fields.

The product-aware solution-class query and exact-SP3 validator are additive.
Existing IGS broadcast-navigation derivation and the legacy center-only
solution-class query retain their signatures. The additive
`default_sample_for_date` query preserves historical GFZ derivation, while the
legacy date-free query now returns GFZ's corrected current `05M` value.
Behavior is deliberately stricter for invalid caller-built identities,
unsupported center/product combinations, pre-transition `cod` long-name
requests, and acquired SP3 bytes that do not meet an exact request. Serialized
SP3 text now includes at least four comment records; blank structural padding
is not returned as semantic `Sp3::comments` text. The new
`ArchiveCompression::UnixCompress` variant and the added typed catalog and
scoreboard error variants are source-visible API additions for exhaustive Rust
matches.

Because this adds public API and makes previously accepted ambiguous or
integrity-invalid inputs fail, it should ship as the next minor release
(`0.33.0`), not as a patch.

## Public evidence for this audit

All sources were accessed on 2026-07-20. Object and directory links record the
archive evidence observed on that date; availability of an individual file is
not a promise that it will remain mirrored by every distributor.

| Catalog or validation decision | Primary public evidence | Accessed |
| --- | --- | --- |
| The official IGS rapid/final orbit combination began on 1994-01-02, GPS week 0730; earlier dates are outside that product series. | [1994 IGS Annual Report, Analysis Centre Coordinator section](https://files.igs.org/pub/resource/pubs/94an_repta.pdf) | 2026-07-20 |
| IGS final, rapid, and ultra-rapid products switched to long filenames at the start of GPS week 2238 on 2022-11-27; final orbit changed from `igs<week><day>.sp3.Z` to `IGS0OPSFIN_<epoch>_01D_15M_ORB.SP3.gz`. November 26 is the final day of week 2237. | [IGS transition guideline](https://files.igs.org/pub/resource/guidelines/Guideline_for_the_transition_of_the_IGS_products_to_IGS20_and_long_filenames_v2.0.pdf), [IGSMAIL-8256](https://lists.igs.org/pipermail/igsmail/2022/008252.html), [IGSMAIL-8274](https://lists.igs.org/pipermail/igsmail/2022/008270.html), [IGS products](https://igs.org/products/) | 2026-07-20 |
| CDDIS documents operational orbit product paths as `WWWW/AAAWWWWD.TYP.Z`, with the GPS-week field represented by four characters. | [NASA CDDIS precise-orbit documentation](https://www.earthdata.nasa.gov/data/space-geodesy-techniques/gnss/precise-orbits-product) | 2026-07-20 |
| CDDIS has the legacy week-2237 final object with Unix-compress packaging and the long-name week-2238 object with gzip packaging. | [week 2237 object](https://cddis.nasa.gov/archive/gnss/products/2237/igs22370.sp3.Z), [week 2238 object](https://cddis.nasa.gov/archive/gnss/products/2238/IGS0OPSFIN_20223310000_01D_15M_ORB.SP3.gz) | 2026-07-20 |
| BKG's current direct layout is `IGS/products/<week>`; its transition-era listings do not support one uniform historical direct rule. | [week 2238 current listing](https://igs.bkg.bund.de/root_ftp/IGS/products/2238/), [week 2235 legacy listing](https://igs.bkg.bund.de/root_ftp/IGS/products/orbits/2235/), [week 2236 long-name listing](https://igs.bkg.bund.de/root_ftp/IGS/products/2236/), [week 2236 legacy listing](https://igs.bkg.bund.de/root_ftp/IGS/products/orbits/2236/) | 2026-07-20 |
| Long-name LEN/SMP syntax documents `D`, `W`, `L`, and `Y` units, while the official archive publishes `07D` despite the guideline's longest-unit prose. Sidereon therefore does not invent `D`-to-`W` or `L`-to-`Y` rewriting. Exact sub-day equivalents such as `60M` and `24H` remain noncanonical, and `00U` is unspecified rather than an exact positive cadence. | [IGS long product filename guidelines v2.2](https://files.igs.org/pub/resource/guidelines/Guidelines_for_Long_Product_Filenames_in_the_IGS_v2.2_EN.pdf), [official week-2420 `07D` product](https://igs.bkg.bund.de/root_ftp/IGS/products/2420/IGS0OPSFIN_20261440000_07D_01D_ERP.ERP.gz) | 2026-07-20 |
| SP3 line 1 declares start and epoch count; line 2 repeats the start as GPS week/seconds-of-week and MJD/fraction and declares an epoch interval strictly between 0 and 100,000 seconds. | [SP3-d specification](https://files.igs.org/pub/data/format/sp3d.pdf) | 2026-07-20 |
| SP3-d requires at least five `+` and five `++` records, at least four header comment records, line-3 satellite-count agreement, a complete ordered satellite record set at every epoch, each V record after its matching P record, and `EOF` as the last record. | [SP3-d specification](https://files.igs.org/pub/data/format/sp3d.pdf) | 2026-07-20 |
| Official SP3 bodies identify their producing agency as `IGS`, `ESOC`, `GFZ`, and `AIUB`; these content fields bind IGS, ESA, GFZ, and CODE catalog identities without assuming the filename producer token is identical. | [IGS rapid SP3](https://igs.bkg.bund.de/root_ftp/IGS/products/2428/IGS0OPSRAP_20262000000_01D_15M_ORB.SP3.gz), [ESA rapid SP3](https://navigation-office.esa.int/products/gnss-products/2428/ESA0OPSRAP_20262000000_01D_05M_ORB.SP3.gz), [GFZ rapid SP3](https://isdc-data.gfz.de/gnss/products/rapid/w2428/GFZ0OPSRAP_20262000000_01D_05M_ORB.SP3.gz), [CODE final SP3](https://www.aiub.unibe.ch/download/CODE_MGEX/CODE/2026/COD0MGXFIN_20261920000_01D_05M_ORB.SP3.gz) | 2026-07-20 |
| AIUB identifies its current product service and CODE product series. | [AIUB services](https://www.aiub.unibe.ch/services/index_eng.html), [CODE Analysis Center](https://www.aiub.unibe.ch/research/code___analysis_center/index_eng.html) | 2026-07-20 |
| AIUB documents operational, rapid, ultra-rapid, predicted, final, MGEX, clock, SP3, and IONEX names and directories. | [AIUB_AFTP.TXT](https://www.aiub.unibe.ch/download/AIUB_AFTP.TXT) | 2026-07-20 |
| Current AIUB listings confirm MGEX final SP3/clock under `CODE_MGEX/CODE/<year>`, final products under `CODE/<year>`, and rapid/ultra-rapid products at `CODE`. | [MGEX 2026 listing](https://code.aiub.unibe.ch/s3_script/aiub_s3_bucket_listing.php?path=CODE_MGEX%2FCODE%2F2026), [CODE 2026 listing](https://code.aiub.unibe.ch/s3_script/aiub_s3_bucket_listing.php?path=CODE%2F2026), [CODE current listing](https://code.aiub.unibe.ch/s3_script/aiub_s3_bucket_listing.php?path=CODE) | 2026-07-20 |
| AIUB's P1 and P2 predicted IONEX tiers are separate paths. | [P1 2026 listing](https://code.aiub.unibe.ch/s3_script/aiub_s3_bucket_listing.php?path=CODE%2FIONO%2FP1%2F2026), [P2 2026 listing](https://code.aiub.unibe.ch/s3_script/aiub_s3_bucket_listing.php?path=CODE%2FIONO%2FP2%2F2026) | 2026-07-20 |
| GFZ rapid SP3 used `15M` through 2021 day 137 and `05M` from day 138 within GPS week 2158; its current rapid series remains `05M`. The current day-200 `05M` object returned HTTP 200 while the corresponding `15M` URL returned 404. | [GFZ week-2158 listing](https://isdc-data.gfz.de/gnss/products/rapid/w2158/), [GFZ current week-2428 listing](https://isdc-data.gfz.de/gnss/products/rapid/w2428/), [current 05M object](https://isdc-data.gfz.de/gnss/products/rapid/w2428/GFZ0OPSRAP_20262000000_01D_05M_ORB.SP3.gz), [absent 15M path](https://isdc-data.gfz.de/gnss/products/rapid/w2428/GFZ0OPSRAP_20262000000_01D_15M_ORB.SP3.gz) | 2026-07-20 |

The archive and format sources above do not document a general permission to
continue after an exact candidate fails integrity validation. This audit found
no narrower moving-alias integrity exception to implement.

## Remaining limits in official material

- BKG publishes historical orbit files, but the reviewed public listings do
  not define one complete rule for directory buckets, filename case, and
  compression across the legacy archive. Sidereon therefore does not derive a
  generic legacy direct-BKG URL.
- No reviewed archive document identifies a moving-alias validation failure
  that is safe to recover from by selecting another product. Only ordinary
  absence has fallback semantics.

The transition date is not unresolved. IGSMAIL-8256, IGSMAIL-8274, the
transition guideline, GPS-week arithmetic, and archive objects agree that week
2238 began on 2022-11-27. The IGS products page's November 26 parenthetical is
an isolated off-by-one statement; the same page identifies that date as the
end of week 2237.

AIUB's legacy CODE names and directories are documented, so they are not an
evidence gap. Supporting them is deferred because they are distinct public
identities requiring product-specific short-name validation and distribution
handling; current long names are never substituted for them.

## Other public evidence

- [NASA CDDIS archive access](https://www.earthdata.nasa.gov/centers/cddis-daac/archive-access)
- [Earthdata Login curl and wget access](https://urs.earthdata.nasa.gov/documentation/for_users/data_access/curl_and_wget)
- [Earthdata bearer-token Python example](https://urs.earthdata.nasa.gov/documentation/for_users/data_access/python_user_token_script)
- [NASA GNSS atmospheric products](https://www.earthdata.nasa.gov/data/space-geodesy-techniques/gnss/atmospheric-products)
- [NASA Earth science data-use policy](https://www.earthdata.nasa.gov/engage/open-data-services-software/data-use-policy)

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

## CDDIS paths

Current long-name SP3 products resolve to:

```text
https://cddis.nasa.gov/archive/gnss/products/<gps-week>/<official-filename>.gz
```

Current long-name IONEX products resolve to:

```text
https://cddis.nasa.gov/archive/gnss/products/ionex/<year>/<day-of-year>/<official-filename>.gz
```

The core rejects CDDIS requests for product families for which this mapping is
not implemented. It does not relabel another file as the requested product.

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

Writes use exclusive temporary files, flush them before atomic promotion, and
make the decompressed data file visible last. Invalid or interrupted first
downloads cannot become cache hits. Per-path in-process locks prevent duplicate
first downloads. A verified existing entry is returned without contacting a
remote service, including in offline mode.

## Compatibility and extension

Legacy direct-fetch APIs and their existing cache locations remain unchanged.
The exact-source API is additive and uses a separate versioned cache tree.
Adding another public distributor requires a location/compression mapping for
an existing identity plus the same redirect, size, content, parse, provenance,
and cache gates. It must not modify identity fields.

This is an additive public API and should ship as the next minor release rather
than a patch release.

## Public evidence

- [NASA CDDIS archive access](https://www.earthdata.nasa.gov/centers/cddis-daac/archive-access)
- [Earthdata Login curl and wget access](https://urs.earthdata.nasa.gov/documentation/for_users/data_access/curl_and_wget)
- [Earthdata bearer-token Python example](https://urs.earthdata.nasa.gov/documentation/for_users/data_access/python_user_token_script)
- [NASA precise orbit products](https://www.earthdata.nasa.gov/data/space-geodesy-techniques/gnss/precise-orbits-product)
- [NASA GNSS atmospheric products](https://www.earthdata.nasa.gov/data/space-geodesy-techniques/gnss/atmospheric-products)
- [IGS long product filename guidelines](https://files.igs.org/pub/resource/guidelines/Guidelines_for_Long_Product_Filenames_in_the_IGS.pdf)
- [NASA Earth science data-use policy](https://www.earthdata.nasa.gov/engage/open-data-services-software/data-use-policy)

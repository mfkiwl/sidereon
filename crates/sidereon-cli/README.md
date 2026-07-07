# sidereon-cli

`sidereon` is a small command-line wrapper over the `sidereon` facade crate. It parses real GNSS products through the library, assembles RINEX observation epochs for SPP through the core convenience API, and prints compact human output or stable JSON where supported.

This v1 intentionally has no TUI and no MCP server.

## Build

From the workspace root:

```sh
cargo build -p sidereon-cli
```

The binary is:

```sh
target/debug/sidereon
```

For a local install into Cargo's bin directory:

```sh
cargo install --path crates/sidereon-cli --locked
```

## Commands

Solve RINEX observation epochs with broadcast navigation:

```sh
sidereon solve --obs data/site.obs --nav data/brdc.rnx
sidereon solve --obs data/site.obs --nav data/brdc.rnx --json
```

Use SP3 precise orbits while retaining broadcast context:

```sh
sidereon solve --obs data/site.obs --nav data/brdc.rnx --sp3 data/orbits.sp3
```

Run RINEX observation lint and observation QC:

```sh
sidereon qc --obs data/site.obs
sidereon qc --obs data/site.obs --json
```

Compute covariance-derived position metrics:

```sh
sidereon metrics --enu-cov "4,0,0,0,9,0,0,0,16"
sidereon metrics --json-file covariance.json --probability 0.99
```

Inspect a GNSS file by trying the supported parsers:

```sh
sidereon inspect data/site.obs
sidereon inspect data/brdc.rnx
sidereon inspect data/orbits.sp3
sidereon inspect data/antennas.atx
sidereon inspect data/stations.tle
```

Exit codes are `0` for success, `1` for parse or solve errors, and `2` for command usage errors.

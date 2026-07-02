#!/usr/bin/env python3
"""Generate independent observation-QC oracle fixtures from raw RINEX OBS text."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
from collections import defaultdict
from pathlib import Path


OBS_FIELD_WIDTH = 16
OBS_VALUE_WIDTH = 14


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=Path(__file__).resolve().parents[3],
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "tests"
        / "fixtures"
        / "qc"
        / "observation_qc_real_oracles.json",
    )
    return parser.parse_args()


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def line_label(line: str) -> str:
    return line[60:].strip() if len(line) > 60 else ""


def parse_obs_codes(line: str) -> tuple[str, int, list[str]]:
    system = line[0]
    count = int(line[3:6])
    codes = line[6:60].split()
    return system, count, codes


def parse_header(lines: list[str]) -> tuple[dict[str, list[str]], float | None, int]:
    obs_codes: dict[str, list[str]] = {}
    expected_counts: dict[str, int] = {}
    current_system: str | None = None
    interval_s: float | None = None

    for idx, line in enumerate(lines):
        label = line_label(line)
        if label == "SYS / # / OBS TYPES":
            head = line[:6].strip()
            if head:
                system, count, codes = parse_obs_codes(line)
                current_system = system
                expected_counts[system] = count
                obs_codes[system] = codes
            elif current_system is not None:
                obs_codes[current_system].extend(line[6:60].split())
        elif label == "INTERVAL":
            interval_s = float(line[:10].strip())
        elif label == "END OF HEADER":
            for system, count in expected_counts.items():
                obs_codes[system] = obs_codes[system][:count]
            return obs_codes, interval_s, idx + 1
    raise ValueError("missing END OF HEADER")


def parse_epoch_header(line: str) -> tuple[dict[str, float | int], int]:
    parts = line.split()
    if parts[0] != ">":
        raise ValueError(f"not an epoch header: {line!r}")
    second = float(parts[6])
    return (
        {
            "year": int(parts[1]),
            "month": int(parts[2]),
            "day": int(parts[3]),
            "hour": int(parts[4]),
            "minute": int(parts[5]),
            "second": second,
            "flag": int(parts[7]),
        },
        int(parts[8]),
    )


def epoch_seconds(epoch: dict[str, float | int]) -> float:
    y = int(epoch["year"])
    m = int(epoch["month"])
    d = int(epoch["day"])
    if m <= 2:
        y -= 1
        m += 12
    a = y // 100
    b = 2 - a + a // 4
    jd = (
        math.floor(365.25 * (y + 4716))
        + math.floor(30.6001 * (m + 1))
        + d
        + b
        - 1524.5
    )
    return (
        jd * 86400.0
        + int(epoch["hour"]) * 3600.0
        + int(epoch["minute"]) * 60.0
        + float(epoch["second"])
    )


def epoch_json(epoch: dict[str, float | int]) -> dict[str, float | int]:
    return {
        "year": int(epoch["year"]),
        "month": int(epoch["month"]),
        "day": int(epoch["day"]),
        "hour": int(epoch["hour"]),
        "minute": int(epoch["minute"]),
        "second": float(epoch["second"]),
    }


def is_sat_record(line: str) -> bool:
    return re.match(r"^[A-Z][ 0-9][0-9]", line) is not None


def read_sat_record(lines: list[str], start: int, n_codes: int) -> tuple[str, int]:
    record = lines[start].rstrip("\n")
    idx = start + 1
    while sat_field_count(record) < n_codes and idx < len(lines):
        nxt = lines[idx].rstrip("\n")
        if nxt.startswith(">") or is_sat_record(nxt):
            break
        record += nxt
        idx += 1
    return record, idx


def sat_field_count(record: str) -> int:
    return max(0, (len(record) - 3 + OBS_FIELD_WIDTH - 1) // OBS_FIELD_WIDTH)


def parse_obs_value(record: str, index: int) -> tuple[float | None, int | None]:
    start = 3 + index * OBS_FIELD_WIDTH
    field = record[start : start + OBS_FIELD_WIDTH].ljust(OBS_FIELD_WIDTH)
    raw_value = field[:OBS_VALUE_WIDTH].strip()
    value = float(raw_value) if raw_value else None
    ssi_raw = field[OBS_VALUE_WIDTH + 1 : OBS_VALUE_WIDTH + 2]
    ssi = int(ssi_raw) if ssi_raw.isdigit() else None
    return value, ssi


class SignalAccum:
    def __init__(self) -> None:
        self.value_observations = 0
        self.ssi = [0] * 10
        self.snr_samples: list[float] = []

    def add(self, code: str, value: float, ssi: int | None) -> None:
        self.value_observations += 1
        self.ssi[0 if ssi is None else min(ssi, 9)] += 1
        if code.startswith("S"):
            self.snr_samples.append(value)

    def finish(self) -> dict[str, object]:
        snr = None
        if self.snr_samples:
            n = len(self.snr_samples)
            mean = sum(self.snr_samples) / n
            if n > 1:
                var = sum((x - mean) ** 2 for x in self.snr_samples) / (n - 1)
                std = math.sqrt(var)
            else:
                std = None
            snr = {
                "n": n,
                "mean": mean,
                "min": min(self.snr_samples),
                "max": max(self.snr_samples),
                "std": std,
            }
        return {
            "value_observations": self.value_observations,
            "ssi": self.ssi if any(self.ssi) else None,
            "snr": snr,
        }


def infer_interval_s(epochs: list[dict[str, float | int]]) -> float | None:
    counts: dict[int, int] = defaultdict(int)
    for a, b in zip(epochs, epochs[1:]):
        delta_ms = round((epoch_seconds(b) - epoch_seconds(a)) * 1000.0)
        if delta_ms > 0:
            counts[delta_ms] += 1
    if not counts:
        return None
    delta_ms = max(counts.items(), key=lambda kv: (kv[1], -kv[0]))[0]
    return delta_ms / 1000.0


def compute_fixture(path: Path, display_path: str) -> dict[str, object]:
    lines = path.read_text().splitlines()
    obs_codes, header_interval_s, body_start = parse_header(lines)
    satellites: dict[str, dict[str, int]] = defaultdict(
        lambda: {"epochs_with_observations": 0, "value_observations": 0}
    )
    satellite_signals: dict[tuple[str, str], SignalAccum] = defaultdict(SignalAccum)
    system_signals: dict[tuple[str, str], SignalAccum] = defaultdict(SignalAccum)
    observation_epochs: list[dict[str, float | int]] = []
    total_epoch_records = 0
    event_records = 0
    power_failure_epochs = 0

    idx = body_start
    while idx < len(lines):
        line = lines[idx]
        if not line.startswith(">"):
            idx += 1
            continue
        total_epoch_records += 1
        epoch, count = parse_epoch_header(line)
        idx += 1
        flag = int(epoch["flag"])
        if flag > 1:
            event_records += 1
            idx += count
            continue
        if flag == 1:
            power_failure_epochs += 1
        observation_epochs.append(epoch)
        for _ in range(count):
            record, next_idx = read_sat_record(lines, idx, 0)
            sat = record[:3].replace(" ", "0")
            system = sat[0]
            codes = obs_codes[system]
            if sat_field_count(record) < len(codes):
                record, next_idx = read_sat_record(lines, idx, len(codes))
            idx = next_idx

            values: list[tuple[str, float, int | None]] = []
            for code_index, code in enumerate(codes):
                value, ssi = parse_obs_value(record, code_index)
                if value is not None:
                    values.append((code, value, ssi))
            if not values:
                continue
            satellites[sat]["epochs_with_observations"] += 1
            satellites[sat]["value_observations"] += len(values)
            for code, value, ssi in values:
                satellite_signals[(sat, code)].add(code, value, ssi)
                system_signals[(system, code)].add(code, value, ssi)

    interval_s = header_interval_s if header_interval_s is not None else infer_interval_s(observation_epochs)
    gaps = []
    if interval_s is not None:
        for start, end in zip(observation_epochs, observation_epochs[1:]):
            delta = epoch_seconds(end) - epoch_seconds(start)
            if delta > interval_s * 1.5:
                gaps.append(
                    {
                        "start_epoch": epoch_json(start),
                        "end_epoch": epoch_json(end),
                        "nominal_interval_s": interval_s,
                        "observed_delta_s": delta,
                        "missing_epochs": int(round(delta / interval_s) - 1),
                    }
                )

    return {
        "path": display_path,
        "sha256": sha256(path),
        "total_epoch_records": total_epoch_records,
        "observation_epochs": len(observation_epochs),
        "event_records": event_records,
        "power_failure_epochs": power_failure_epochs,
        "skipped_records": 0,
        "interval_s": interval_s,
        "missing_epochs": sum(gap["missing_epochs"] for gap in gaps),
        "data_gaps": gaps,
        "satellites": [
            {"satellite": sat, **values} for sat, values in sorted(satellites.items())
        ],
        "satellite_signals": [
            {"satellite": sat, "code": code, **acc.finish()}
            for (sat, code), acc in sorted(satellite_signals.items())
        ],
        "system_signals": [
            {"system": system, "code": code, **acc.finish()}
            for (system, code), acc in sorted(system_signals.items())
        ],
    }


def main() -> None:
    args = parse_args()
    manifest_dir = args.repo_root / "crates" / "sidereon-core"
    rel_paths = [
        "tests/fixtures/obs/ESBC00DNK_R_20201770000_01D_30S_MO_120epoch.rnx",
        "tests/fixtures/obs/ZIM200CHE_R_20261330000_01H_30S_MO_120epoch.rnx",
    ]
    fixtures = [compute_fixture(manifest_dir / rel, rel) for rel in rel_paths]
    doc = {
        "provenance": {
            "generator": "crates/sidereon-core/fixtures-generators/generate_observation_qc_oracles.py",
            "method": "Independent fixed-column extraction from raw RINEX OBS text; does not call sidereon parsers or QC code.",
            "generated_for": "QC oracle bar, 2026-07-02",
        },
        "fixtures": fixtures,
    }
    args.output.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()

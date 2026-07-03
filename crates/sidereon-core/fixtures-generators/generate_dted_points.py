#!/usr/bin/env python3
"""Generate DTED lookup fixtures from public terrain tile identifiers."""

from __future__ import annotations

import json
import struct
from pathlib import Path

UHL_SIZE = 80
DSI_SIZE = 648
ACC_SIZE = 2700
DATA_OFFSET = UHL_SIZE + DSI_SIZE + ACC_SIZE
LON_COUNT = 5
LAT_COUNT = 5


class SyntheticTileSpec:
    def __init__(self, tile_id: str, lon0: float, lat0: float, formula: str, variant: str):
        self.tile_id = tile_id
        self.tile_name = f"{tile_id}_1arc_v3.dt2"
        self.lon0 = lon0
        self.lat0 = lat0
        self.formula = formula
        self.variant = variant

    def elevation(self, lon_i: int, lat_i: int) -> int:
        if self.variant == "primary":
            return -20 + 7 * lon_i - 5 * lat_i + lon_i * lat_i
        if self.variant == "east":
            return 100 + 3 * lon_i + 11 * lat_i - lon_i * lat_i
        raise ValueError(f"unknown synthetic DTED variant {self.variant}")


SYNTHETIC_TILES = [
    SyntheticTileSpec(
        "n36_w107",
        -107.0,
        36.0,
        "z_m = -20 + 7 * lon_i - 5 * lat_i + lon_i * lat_i",
        "primary",
    ),
    SyntheticTileSpec(
        "n36_w106",
        -106.0,
        36.0,
        "z_m = 100 + 3 * lon_i + 11 * lat_i - lon_i * lat_i",
        "east",
    ),
]


def f64_bits(value: float) -> str:
    return f"0x{struct.unpack('<Q', struct.pack('<d', float(value)))[0]:016x}"


def signed_magnitude(raw: int) -> int:
    if raw & 0x8000:
        return -int(raw & 0x7FFF)
    return int(raw)


def parse_ascii_int(buf: bytes) -> int:
    return int(buf.decode("ascii").strip())


def parse_coord(text: str) -> float:
    hemi = text[-1]
    sign = -1.0 if hemi in {"S", "W"} else 1.0
    coord = text[:-1]
    sec_idx = len(coord) - 4 if coord[-2] == "." else len(coord) - 2
    min_idx = sec_idx - 2
    degree = int(coord[:min_idx])
    minute = int(coord[min_idx:sec_idx])
    second = float(coord[sec_idx:])
    return sign * (degree + (minute + second / 60.0) / 60.0)


def dted_coord(value: float, is_lon: bool) -> bytes:
    hemi = ("E" if value >= 0.0 else "W") if is_lon else ("N" if value >= 0.0 else "S")
    value = abs(value)
    degree = int(value)
    minute_float = (value - degree) * 60.0
    minute = int(minute_float)
    second = (minute_float - minute) * 60.0
    return f"{degree:03}{minute:02}{second:02.0f}{hemi}".encode("ascii")


def signed_magnitude_bytes(value: int) -> bytes:
    if value < 0:
        raw = 0x8000 | abs(value)
    else:
        raw = value
    return raw.to_bytes(2, byteorder="big", signed=False)


def write_synthetic_tile(path: Path, spec: SyntheticTileSpec) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    uhl = bytearray(b" " * UHL_SIZE)
    uhl[0:4] = b"UHL1"
    uhl[4:12] = dted_coord(spec.lon0, is_lon=True)
    uhl[12:20] = dted_coord(spec.lat0, is_lon=False)
    uhl[47:51] = f"{LON_COUNT:04}".encode("ascii")
    uhl[51:55] = f"{LAT_COUNT:04}".encode("ascii")

    data = bytearray()
    for lon_i in range(LON_COUNT):
        block = bytearray(12 + 2 * LAT_COUNT)
        block[0] = 0xAA
        block[4:6] = lon_i.to_bytes(2, byteorder="big")
        for lat_i in range(LAT_COUNT):
            sample = 8 + lat_i * 2
            block[sample : sample + 2] = signed_magnitude_bytes(
                spec.elevation(lon_i, lat_i)
            )
        checksum = sum(block[:-4])
        block[-4:] = checksum.to_bytes(4, byteorder="big", signed=True)
        data.extend(block)

    path.write_bytes(bytes(uhl) + (b" " * DSI_SIZE) + (b" " * ACC_SIZE) + bytes(data))


class Tile:
    def __init__(self, path: Path):
        self.path = path
        self.data = path.read_bytes()
        if self.data[:4] != b"UHL1":
            raise ValueError(f"{path} missing UHL1 header")
        self.lon0 = parse_coord(self.data[4:12].decode("ascii"))
        self.lat0 = parse_coord(self.data[12:20].decode("ascii"))
        self.lon_count = parse_ascii_int(self.data[47:51])
        self.lat_count = parse_ascii_int(self.data[51:55])
        self.data_offset = 80 + 648 + 2700
        self.block_len = 12 + 2 * self.lat_count

    def elevation(self, lon: float, lat: float) -> int:
        lon_idx = round((lon - self.lon0) * (self.lon_count - 1))
        lat_idx = round((lat - self.lat0) * (self.lat_count - 1))
        block = self.data[self.data_offset + lon_idx * self.block_len : self.data_offset + (lon_idx + 1) * self.block_len]
        sample = 8 + lat_idx * 2
        raw = int.from_bytes(block[sample : sample + 2], byteorder="big", signed=False)
        return signed_magnitude(raw)

    def bilinear(self, lon: float, lat: float) -> float:
        lon_idx = (lon - self.lon0) * (self.lon_count - 1)
        lat_idx = (lat - self.lat0) * (self.lat_count - 1)
        lon_lo = int(lon_idx // 1)
        lat_lo = int(lat_idx // 1)
        fx = lon_idx - lon_lo
        fy = lat_idx - lat_lo
        z = 0.0
        for di, wx in ((0, 1.0 - fx), (1, fx)):
            for dj, wy in ((0, 1.0 - fy), (1, fy)):
                w = wx * wy
                if w == 0.0:
                    continue
                posting_lon = self.lon0 + (lon_lo + di) / (self.lon_count - 1)
                posting_lat = self.lat0 + (lat_lo + dj) / (self.lat_count - 1)
                z += w * self.elevation(posting_lon, posting_lat)
        return z


def main() -> None:
    out_dir = Path(__file__).resolve().parents[1] / "tests" / "fixtures" / "dted"
    tile_paths = {}
    for spec in SYNTHETIC_TILES:
        tile_path = out_dir / "tiles" / spec.tile_name
        write_synthetic_tile(tile_path, spec)
        tile_paths[spec.tile_id] = tile_path

    primary_spec = SYNTHETIC_TILES[0]
    tile = Tile(tile_paths[primary_spec.tile_id])
    nearest_cases = []
    fractions = [0.0, 0.25, 0.5, 0.75, 1.0]
    for lon_frac in fractions:
        for lat_frac in fractions:
            lon = tile.lon0 + lon_frac
            lat = tile.lat0 + lat_frac
            nearest_cases.append(
                {
                    "tile_id": primary_spec.tile_id,
                    "longitude_bits": f64_bits(lon),
                    "latitude_bits": f64_bits(lat),
                    "elevation_bits": f64_bits(float(tile.elevation(lon, lat))),
                }
            )

    bilinear_cases = []
    for lon_frac, lat_frac in ((0.125, 0.125), (0.375, 0.625), (0.625, 0.375), (0.875, 0.875)):
        lon = tile.lon0 + lon_frac
        lat = tile.lat0 + lat_frac
        bilinear_cases.append(
            {
                "tile_id": primary_spec.tile_id,
                "longitude_bits": f64_bits(lon),
                "latitude_bits": f64_bits(lat),
                "elevation_bits": f64_bits(tile.bilinear(lon, lat)),
            }
        )

    tile_by_id = {
        spec.tile_id: Tile(tile_paths[spec.tile_id])
        for spec in SYNTHETIC_TILES
    }
    multi_tile_cases = []
    for case_id, tile_id, lon_frac, lat_frac in (
        ("a_interior_1", "n36_w107", 0.125, 0.125),
        ("a_interior_2", "n36_w107", 0.625, 0.375),
        ("b_interior_1", "n36_w106", 0.125, 0.125),
        ("b_interior_2", "n36_w106", 0.625, 0.375),
        ("shared_meridian_from_b", "n36_w106", 0.0, 0.5),
    ):
        case_tile = tile_by_id[tile_id]
        lon = case_tile.lon0 + lon_frac
        lat = case_tile.lat0 + lat_frac
        multi_tile_cases.append(
            {
                "case_id": case_id,
                "tile_id": tile_id,
                "longitude_bits": f64_bits(lon),
                "latitude_bits": f64_bits(lat),
                "nearest_bits": f64_bits(float(case_tile.elevation(lon, lat))),
                "bilinear_bits": f64_bits(case_tile.bilinear(lon, lat)),
            }
        )

    payload = {
        "schema": "gnss-dted-points-v1",
        "source": {
            "tile_id": primary_spec.tile_id,
            "tile_path": f"tiles/{primary_spec.tile_name}",
            "format": "DTED UHL/DSI/ACC/data-record byte layout",
            "elevation_formula": primary_spec.formula,
        },
        "tiles": [
            {
                "tile_id": spec.tile_id,
                "tile_path": f"tiles/{spec.tile_name}",
                "elevation_formula": spec.formula,
            }
            for spec in SYNTHETIC_TILES
        ],
        "nearest_cases": nearest_cases,
        "bilinear_cases": bilinear_cases,
        "multi_tile_cases": multi_tile_cases,
    }
    out = Path(__file__).resolve().parents[1] / "tests" / "fixtures" / "dted" / "dted_points.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    print(f"wrote {out}")


if __name__ == "__main__":
    main()

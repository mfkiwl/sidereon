#!/usr/bin/env python3
"""Generate the dense PROJ 9.3.0 EGM96 bit oracle used by geoid.rs tests.

Only public inputs are used: an AArch64 contracted-arithmetic build of PROJ tag
9.3.0, OSGeo's egm96_15.gtx, and pyproj as a second PROJ build/version check.
"""

from __future__ import annotations

import argparse
import ctypes
import math
import struct
from pathlib import Path


MAGIC = b"SIDGEO93"
DEG_TO_RAD = 0.017453292519943296


class PjInfo(ctypes.Structure):
    _fields_ = [
        ("major", ctypes.c_int),
        ("minor", ctypes.c_int),
        ("patch", ctypes.c_int),
        ("release", ctypes.c_char_p),
        ("version", ctypes.c_char_p),
        ("searchpath", ctypes.c_char_p),
        ("paths", ctypes.POINTER(ctypes.c_char_p)),
        ("path_count", ctypes.c_size_t),
    ]


class PjXyzt(ctypes.Structure):
    _fields_ = [(name, ctypes.c_double) for name in ("x", "y", "z", "t")]


class PjCoord(ctypes.Union):
    _fields_ = [("v", ctypes.c_double * 4), ("xyzt", PjXyzt)]


def bits(value: float) -> int:
    return struct.unpack(">Q", struct.pack(">d", value))[0]


def dense_points() -> list[tuple[float, float]]:
    points = [
        (-180.0, -90.0),
        (180.0, -90.0),
        (-180.0, 90.0),
        (180.0, 90.0),
        (-540.0, 0.0),
        (540.0, 0.0),
        (-180.000001, -10.5),
        (180.000001, -10.5),
        (0.0, 0.0),
        (-122.4194, 37.7749),
    ]
    for lat_i in range(81):
        lat_deg = -89.999 + lat_i * (179.998 / 80.0)
        for lon_i in range(161):
            lon_deg = -179.999 + lon_i * (359.998 / 160.0)
            points.append((lon_deg, lat_deg))
    return [(lon * DEG_TO_RAD, lat * DEG_TO_RAD) for lon, lat in points]


def load_proj(path: Path) -> ctypes.CDLL:
    lib = ctypes.CDLL(str(path))
    lib.proj_info.restype = PjInfo
    info = lib.proj_info()
    version = (info.major, info.minor, info.patch)
    if version != (9, 3, 0):
        raise RuntimeError(f"oracle requires PROJ 9.3.0, loaded {version}")
    lib.proj_context_create.restype = ctypes.c_void_p
    lib.proj_context_destroy.argtypes = [ctypes.c_void_p]
    lib.proj_create.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    lib.proj_create.restype = ctypes.c_void_p
    lib.proj_destroy.argtypes = [ctypes.c_void_p]
    lib.proj_coord.argtypes = [ctypes.c_double] * 4
    lib.proj_coord.restype = PjCoord
    lib.proj_trans.argtypes = [ctypes.c_void_p, ctypes.c_int, PjCoord]
    lib.proj_trans.restype = PjCoord
    return lib


def proj_values(lib: ctypes.CDLL, gtx_path: Path, points: list[tuple[float, float]]) -> list[float]:
    context = lib.proj_context_create()
    definition = (
        "+proj=pipeline +step +inv +proj=vgridshift "
        f"+grids={gtx_path.resolve()} +multiplier=1"
    ).encode()
    pipeline = lib.proj_create(context, definition)
    if not pipeline:
        raise RuntimeError("PROJ could not create the EGM96 vgridshift pipeline")
    try:
        values = []
        for lon_rad, lat_rad in points:
            coordinate = lib.proj_coord(lon_rad, lat_rad, 0.0, 0.0)
            output = lib.proj_trans(pipeline, 1, coordinate)  # PJ_FWD
            values.append(-output.v[2])
        return values
    finally:
        lib.proj_destroy(pipeline)
        lib.proj_context_destroy(context)


def pyproj_cross_check(gtx_path: Path, points: list[tuple[float, float]], expected: list[float]) -> str:
    import pyproj

    transformer = pyproj.Transformer.from_pipeline(
        "+proj=pipeline +step +inv +proj=vgridshift "
        f"+grids={gtx_path.resolve()} +multiplier=1"
    )
    lons, lats = zip(*points)
    _, _, zs = transformer.transform(lons, lats, [0.0] * len(points), radians=True)
    for index, (z, want) in enumerate(zip(zs, expected)):
        if bits(-z) != bits(want):
            raise RuntimeError(
                f"pyproj differs from PROJ 9.3.0 at point {index}: "
                f"{bits(-z):016x} != {bits(want):016x}"
            )
    return f"pyproj {pyproj.__version__} / PROJ {pyproj.proj_version_str}"


def required_nodes(gtx: bytes, points: list[tuple[float, float]]) -> dict[int, bytes]:
    south, west, res_y, res_x, height, width = struct.unpack(">ddddii", gtx[:40])
    west_rad = west * DEG_TO_RAD
    south_rad = south * DEG_TO_RAD
    res_x_rad = res_x * DEG_TO_RAD
    res_y_rad = res_y * DEG_TO_RAD
    east_rad = (west + res_x * (width - 1)) * DEG_TO_RAD
    inv_res_x = 1.0 / res_x_rad
    inv_res_y = 1.0 / res_y_rad
    full_world = east_rad - west_rad + res_x_rad >= 2.0 * math.pi - 1.0e-10
    nodes: dict[int, bytes] = {}
    for lon_rad, lat_rad in points:
        grid_x = (lon_rad - west_rad) * inv_res_x
        if lon_rad < west_rad or lon_rad > east_rad:
            if full_world:
                grid_x = math.fmod(
                    math.fmod(grid_x + width, width) + width,
                    width,
                )
            elif lon_rad < west_rad:
                grid_x = (lon_rad + 2.0 * math.pi - west_rad) * inv_res_x
            else:
                grid_x = (lon_rad - 2.0 * math.pi - west_rad) * inv_res_x
        grid_y = (lat_rad - south_rad) * inv_res_y
        ix = math.floor(grid_x)
        iy = math.floor(grid_y)
        ix2 = 0 if ix + 1 >= width and full_world else min(ix + 1, width - 1)
        iy2 = min(iy + 1, height - 1)
        for row, col in ((iy, ix), (iy, ix2), (iy2, ix), (iy2, ix2)):
            index = row * width + col
            offset = 40 + index * 4
            nodes[index] = gtx[offset : offset + 4]
    return nodes


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--proj-lib", required=True, type=Path)
    parser.add_argument("--gtx", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()

    gtx = args.gtx.read_bytes()
    if len(gtx) != 40 + 721 * 1440 * 4:
        raise RuntimeError("unexpected egm96_15.gtx byte length")
    digest = __import__("hashlib").sha256(gtx).hexdigest()
    if digest != "c02a6eb70a7a78efebe5adf3ade626eb75390e170bb8b3f36136a2c28f5326a0":
        raise RuntimeError(f"unexpected egm96_15.gtx SHA-256: {digest}")

    points = dense_points()
    expected = proj_values(load_proj(args.proj_lib), args.gtx, points)
    checker = pyproj_cross_check(args.gtx, points, expected)
    nodes = required_nodes(gtx, points)

    payload = bytearray(MAGIC)
    payload.extend(struct.pack(">II", len(nodes), len(points)))
    for index, sample in sorted(nodes.items()):
        payload.extend(struct.pack(">I", index))
        payload.extend(sample)
    for (lon_rad, lat_rad), value in zip(points, expected):
        payload.extend(struct.pack(">QQQ", bits(lon_rad), bits(lat_rad), bits(value)))
    args.output.write_bytes(payload)
    print(
        f"wrote {args.output}: {len(points)} points, {len(nodes)} nodes, "
        f"{len(payload)} bytes; cross-checked with {checker}"
    )


if __name__ == "__main__":
    main()

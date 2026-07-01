"""Generate the almanac SPK excerpt and reference instants.

Requires skyfield 1.54 and jplephem 2.24.
Source kernel: de421.bsp, SHA256
a20a7139da04cbc462454634918e9a9ca69127044e2cc9d4f9c16e238d2deedc.
Generated fixture: almanac_de421.spk, SHA256
0979ec29c44fdd60411158283eac626d6b11bd100945652125ca35bf9114177d.
Generation date: 2026-07-01.

Cross-check sources:
USNO seasons: https://aa.usno.navy.mil/data/seasons
USNO Moon phases: https://aa.usno.navy.mil/data/MoonPhases
NASA eclipse catalog: https://eclipse.gsfc.nasa.gov/
EclipseWise solar table: https://eclipsewise.com/solar/SEprime/2001-2100/SE2026Aug12Tprime.html
"""

from pathlib import Path
import subprocess
import sys

from skyfield.api import load
from skyfield import almanac

HERE = Path(__file__).resolve().parent
OUTPUT = HERE / "almanac_de421.spk"
TARGETS = "1,2,3,4,5,6,7,8,10,301,399"


def print_instant(label, time):
    dt = time.utc_datetime()
    unix_us = int(round(dt.timestamp() * 1_000_000))
    print(f"{label}: {time.utc_iso()} unix_us={unix_us}")


def rebuild_excerpt(full_de421):
    subprocess.run(
        [
            sys.executable,
            "-m",
            "jplephem",
            "excerpt",
            "--targets",
            TARGETS,
            "2024/12/01",
            "2026/09/30",
            str(full_de421),
            str(OUTPUT),
        ],
        check=True,
    )


def main():
    if len(sys.argv) == 2:
        rebuild_excerpt(Path(sys.argv[1]))

    ts = load.timescale()
    eph = load(str(OUTPUT))

    start_2025 = ts.utc(2025, 1, 1)
    end_2026 = ts.utc(2026, 1, 1)
    times, kinds = almanac.find_discrete(start_2025, end_2026, almanac.seasons(eph))
    season_names = ["march", "june", "september", "december"]
    for time, kind in zip(times, kinds):
        print_instant(f"season_{season_names[kind]}", time)

    times, kinds = almanac.find_discrete(
        ts.utc(2025, 1, 1), ts.utc(2025, 2, 15), almanac.moon_phases(eph)
    )
    phase_names = ["new", "first", "full", "last"]
    for time, kind in zip(times, kinds):
        print_instant(f"phase_{phase_names[kind]}", time)

    for name in ["mars barycenter", "saturn barycenter", "venus barycenter"]:
        function = almanac.oppositions_conjunctions(eph, eph[name])
        times, kinds = almanac.find_discrete(ts.utc(2025, 1, 1), ts.utc(2025, 12, 31), function)
        for time, kind in zip(times, kinds):
            event = "opposition" if kind else "conjunction"
            print_instant(f"{name.replace(' ', '_')}_{event}", time)

    print("lunar_total_2025_cross_check: 2025-03-14T06:59Z")
    print("solar_total_2026_cross_check: 2026-08-12T17:46Z gamma=0.8978")


if __name__ == "__main__":
    main()

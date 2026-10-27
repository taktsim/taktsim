"""Regenerate every table, figure and data file of the concept pages,
and website/theme/tokens.css. CI runs this and fails if anything changed.

Usage: python3 docs/concepts/scripts/build_all.py
"""

import importlib
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from common import write  # noqa: E402
from svgkit import ROOT, tokens_css  # noqa: E402

PAGES = [
    "p01_production_systems",
    "p02_bottleneck",
    "p03_littles_law",
    "p04_variability",
    "p05_utilization",
    "p06_starving_blocking",
    "p07_buffers",
    "p08_unreliable",
    "p09_longer_lines",
    "p10_takt_pull",
    "p11_batching",
    "p12_scrap_rework",
    "p13_trust",
    "p14_reality",
]


def main() -> None:
    write(ROOT / "website" / "theme" / "tokens.css", tokens_css())
    for name in PAGES:
        start = time.perf_counter()
        importlib.import_module(name).build()
        print(f"{name}: {time.perf_counter() - start:.1f} s", flush=True)


if __name__ == "__main__":
    main()

"""Stage C's statistic: match.py's 14 image statistics at the references' organism scale
(README.md beside this file, "Stage C").

    python3 scale.py reference OUT.json IMG:ZOOM ...     the statistics of screenshots at their zooms
    python3 scale.py score REF.json DIAMETER_PX IMG ...  each image's distance from REF.json

match.py compares frames at one pixel per field cell, which suited the references' wide views. The
second set of reference screenshots is zoomed in 2.2-26x, where single organisms show as spheres. Here
every image is first resized so that one organism is TARGET pixels across, and only then cut into
match.py's 128-pixel patches and measured, so a reference and a frame are compared at the same number
of organisms a patch whatever their zoom or dot size:
- a screenshot at zoom z (read from its overlay) draws an organism WIDTH * z pixels across; only those
  at zoom 5 or less leave a whole patch after resizing;
- a frame of a contact world draws an organism at its body diameter: DIAMETER_PX = the frame's pixels
  per world unit times [contact] diameter.
The distance is match.py's: the root mean square of each statistic's difference from the references'
mean, in units of its spread across the reference patches. Lower is closer.

The screenshots are third-party and stay out of git (AGENTS.md); scale-reference.json holds only the
statistics measured from them. Nothing this computes feeds back into any run.
"""

import importlib.util
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("match", HERE / "match.py")
match = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(match)

# The screenshots' organism width in pixels at zoom 1: their zoom-26.3 view draws spheres ~20 px across.
WIDTH = 0.77
# An organism's width in pixels once resized.
TARGET = 2.0
# The widest zoom a screenshot may have and still leave a whole patch.
MAXIMUM_ZOOM = 5.0


def resized(image: Image.Image, diameter: float) -> np.ndarray:
    scale = TARGET / diameter
    width, height = image.size
    out = image.resize((max(1, round(width * scale)), max(1, round(height * scale))), Image.LANCZOS)
    return np.asarray(out.convert("RGB"), dtype=np.float64)


def patches(rgb: np.ndarray):
    for y in range(0, rgb.shape[0] - match.PATCH + 1, match.PATCH):
        for x in range(0, rgb.shape[1] - match.PATCH + 1, match.PATCH):
            yield rgb[y:y + match.PATCH, x:x + match.PATCH]


def reference(out: Path, items) -> None:
    rows, zooms = [], []
    for item in items:
        path, zoom = item.rsplit(":", 1)
        zoom = float(zoom)
        if zoom > MAXIMUM_ZOOM:
            continue
        with Image.open(path) as opened:
            width, height = opened.size
            # Less the player's caption row, its clock and the rounded frame.
            body = opened.convert("RGB").crop((14, 30, width - 14, height - 50))
            found = [match.features(p) for p in patches(resized(body, WIDTH * zoom))]
        rows += found
        zooms.append([zoom, len(found)])
    table = np.array(rows)
    mean = table.mean(axis=0)
    spread = np.maximum(table.std(axis=0), match.FLOORS)
    out.write_text(json.dumps({
        "names": match.NAMES,
        "organism_pixels": TARGET,
        "patches": len(rows),
        "zooms_and_patches": zooms,
        "mean": [round(float(v), 4) for v in mean],
        "spread": [round(float(v), 4) for v in spread],
    }, indent=1) + "\n")
    print(f"{len(rows)} patches from {len(zooms)} images -> {out}")
    for name, m, s in zip(match.NAMES, mean, spread):
        print(f"  {name:22} {m:8.3f} ± {s:.3f}")


def score(ref: dict, diameter: float, path: Path):
    """The distance of one frame, and its mean statistics."""
    with Image.open(path) as opened:
        rgb = resized(opened.convert("RGB"), diameter)
    mean = np.array([match.features(p) for p in patches(rgb)]).mean(axis=0)
    return match.distance(mean, ref), mean


if __name__ == "__main__":
    if sys.argv[1] == "reference":
        reference(Path(sys.argv[2]), sys.argv[3:])
    elif sys.argv[1] == "score":
        ref = json.loads(Path(sys.argv[2]).read_text())
        diameter = float(sys.argv[3])
        for name in sys.argv[4:]:
            d, mean = score(ref, diameter, Path(name))
            z = (mean - np.array(ref["mean"])) / np.array(ref["spread"])
            worst = sorted(zip(np.abs(z), match.NAMES), reverse=True)[:3]
            print(f"{d:.3f}  {name}  worst: " + ", ".join(f"{n} {v:.1f}" for v, n in worst))
    else:
        raise SystemExit(__doc__)

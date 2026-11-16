"""Stage W of the showcase search: configurations scored by how closely their frames' image
statistics match the reference screenshots' (README.md beside this file).

    FLIFE=... SHOWCASE_LAB=... python3 match.py COMMAND ...

    reference OUT.json IMG...        the reference statistics from zoom-1 screenshots (1 px ~ 1 field cell)
    run FIRST COUNT [PARALLEL]       sample (`sample_wide`) and run configurations FIRST..FIRST+COUNT
    signal FIRST COUNT [PARALLEL]    the same with `sample_signal` (emission-dominated worlds)
    local P1,P2,... FIRST COUNT [PAR] perturbations of the named samples, as search.py's `local`
    score NAME                       (re)score one sample against reference.json
    rescore                          rescore every finished sample (after reference.json changes)
    rank [N]                         every scored sample by distance; contact sheet of the best N

The reference screenshots are third-party and stay out of git (AGENTS.md); `reference.json`
holds only the statistics measured from them. The search is outside the world: it reads a
run's frames after the run, and nothing it computes feeds back into any run.

The statistics, on a frame reduced to one pixel per field cell (the references show a
~1,000-cell world at zoom 1 in ~1,040 pixels), with the background removed and the
luminance divided by its mean, so neither brightness nor dot size sets the scale:
- coverage: the share of cells lit above 0.3 of the mean after a 1-cell blur;
- voids: the share of cells below 0.15 of the mean after a 3-cell blur (large dark holes);
- contrast: log10 of the normalized luminance's variance;
- shape: log10 of the luminance power in 8 wavelength bands from 2 to 64 cells, less their
  mean, so the spectrum's shape (the mesh, the holes, the bodies), not its level, which
  contrast already measures (a sparse image has more power in every band);
- coherence: the cosine similarity of chromaticity between lit cells 8, 16 and 32 cells
  apart (how far a colour, a lineage's territory, extends).
Both a reference and a frame are blurred by 0.7 cells first, and their chromaticity by 1.5
cells more, as a video's 4:2:0, compressed chroma is (`features`).
The distance is the root mean square of each statistic's difference from the references'
mean in units of its spread across the reference patches (floored, so no one statistic
dominates); lower is closer.
"""

import importlib.util
import json
import math
import random
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.ndimage import gaussian_filter

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("search", HERE / "search.py")
base = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(base)

PATCH = 128
EDGES = [2.0, 3.0, 4.5, 7.0, 11.0, 17.0, 26.0, 40.0, 64.0]
SHIFTS = [8, 16, 32]
# Floors on each statistic's spread: coverage, voids and contrast, the spectrum's shape,
# coherence.
FLOORS = [0.05, 0.05, 0.1] + [0.1] * (len(EDGES) - 1) + [0.05] * len(SHIFTS)
NAMES = ["coverage", "voids", "contrast"] + [f"shape_{a:g}_{b:g}" for a, b in zip(EDGES, EDGES[1:])] + [
    f"coherence_{d}" for d in SHIFTS]


def features(rgb: np.ndarray) -> np.ndarray:
    """The statistics of an H x W x 3 image at one pixel per cell (not wrapped).

    Both sides are first blurred by 0.7 cells: the references are frames of a compressed
    video (4:2:0 chroma, smoothed fine detail), and a render compared raw would differ from
    them by its sharpness alone. Coherence starts at 8 cells for the same reason."""
    rgb = gaussian_filter(rgb.astype(np.float64), sigma=(0.7, 0.7, 0))
    lum = rgb.mean(axis=2)
    lit = np.clip(lum - np.percentile(lum, 5), 0, None)
    mean = lit.mean()
    if mean <= 0:
        return np.zeros(len(NAMES))
    norm = lit / mean
    coverage = float((gaussian_filter(norm, 1) > 0.3).mean())
    voids = float((gaussian_filter(norm, 3) < 0.15).mean())
    contrast = math.log10(max(float(norm.var()), 1e-12))
    f = np.fft.fft2(norm - norm.mean())
    power = (np.abs(f) ** 2) / norm.size
    ky = np.fft.fftfreq(norm.shape[0])[:, None]
    kx = np.fft.fftfreq(norm.shape[1])[None, :]
    k = np.sqrt(kx * kx + ky * ky)
    with np.errstate(divide="ignore"):
        wavelength = np.where(k > 0, 1.0 / k, np.inf)
    spectrum = []
    for lo, hi in zip(EDGES, EDGES[1:]):
        band = (wavelength >= lo) & (wavelength < hi)
        spectrum.append(math.log10(max(float(power[band].mean()), 1e-12)))
    shape = np.array(spectrum) - np.mean(spectrum)
    # Chromaticity: each lit cell's colour less grey, as a unit-free 3-vector, smoothed as
    # the video's chroma is.
    total = rgb.sum(axis=2, keepdims=True)
    chroma = np.where(total > 0, rgb / np.maximum(total, 1e-9) - 1.0 / 3.0, 0.0)
    chroma = gaussian_filter(chroma, sigma=(1.5, 1.5, 0))
    weight = np.clip(norm, 0, 2)
    coherence = []
    for d in SHIFTS:
        num = den = 0.0
        for a, b, wa, wb in [
            (chroma[:, :-d], chroma[:, d:], weight[:, :-d], weight[:, d:]),
            (chroma[:-d, :], chroma[d:, :], weight[:-d, :], weight[d:, :]),
        ]:
            w = wa * wb
            num += float((w * (a * b).sum(axis=2)).sum())
            den += float((w * np.sqrt((a * a).sum(axis=2) * (b * b).sum(axis=2))).sum())
        coherence.append(num / den if den > 0 else 0.0)
    return np.array([coverage, voids, contrast, *shape, *coherence])


def cells(path: Path, side: int) -> np.ndarray:
    """A frame of a side x side world as one RGB pixel per cell (area mean)."""
    with Image.open(path) as opened:
        image = np.asarray(opened.convert("RGB"), dtype=np.float64)
    k = image.shape[0] // side
    return image.reshape(side, k, side, k, 3).mean(axis=(1, 3))


def reference(out: Path, images) -> None:
    """Statistics of every PATCH x PATCH patch of each screenshot, less its overlays: the
    caption row at the top, the player's controls at the bottom, the rounded frame."""
    rows = []
    for path in images:
        with Image.open(path) as opened:
            image = np.asarray(opened.convert("RGB"), dtype=np.float64)
        height, width = image.shape[:2]
        body = image[50:height - 150, 40:width - 40]
        for y in range(0, body.shape[0] - PATCH + 1, PATCH):
            for x in range(0, body.shape[1] - PATCH + 1, PATCH):
                rows.append(features(body[y:y + PATCH, x:x + PATCH]))
    table = np.array(rows)
    mean = table.mean(axis=0)
    spread = np.maximum(table.std(axis=0), FLOORS)
    out.write_text(json.dumps({
        "names": NAMES,
        "patches": len(rows),
        "images": len(images),
        "mean": [round(float(v), 4) for v in mean],
        "spread": [round(float(v), 4) for v in spread],
    }, indent=1) + "\n")
    print(f"{len(rows)} patches from {len(images)} images -> {out}")
    for name, m, s in zip(NAMES, mean, spread):
        print(f"  {name:22} {m:8.3f} ± {s:.3f}")


def distance(stats: np.ndarray, ref: dict) -> float:
    z = (stats - np.array(ref["mean"])) / np.array(ref["spread"])
    return float(np.sqrt(np.mean(z * z)))


def sample_wide(index: int) -> dict:
    """A configuration from random.Random(200000 + index). Unlike stage 1's `sample`, the
    population quota binds: `refuse-excess-births-v1` (plan §7.5's exhibition policy) holds
    the world at its cap instead of pausing, as the reference's (pop ~299,990 throughout).
    Reserves (the attempt threshold) reach 40 energy, not 4; the supply is broader; the
    sensing range, the mutation scales and the cap density are drawn too."""
    rng = random.Random(200000 + index)
    logu = base.logu
    area = base.SIDE * base.SIDE
    count = rng.choice([8, 16, 32, 64, 128])
    supply = logu(rng, 0.5, 8.0)  # energy per cell per second, all sources together
    cap = int(logu(rng, 0.4, 3.0) * area)
    reserve = logu(rng, 2.0, 40.0)
    return {
        ("model", "model"): rng.choice(["field-ecology-v1", "field-ecology-v1-ungated-motion"]),
        ("world", "diffusion"): logu(rng, 0.02, 1.5),
        ("world", "decay_per_second"): logu(rng, 0.003, 0.3),
        ("sources", "count"): count,
        ("sources", "sigma_world_units"): logu(rng, 4.0, 40.0),
        ("sources", "power_per_source"): supply * area / count,
        ("sources", "path_speed"): logu(rng, 0.2, 8.0),
        ("chemistry", "toxicity_sensitivity"): logu(rng, 0.02, 1.0),
        ("chemistry", "discrimination_threshold"): rng.uniform(0.05, 0.45),
        ("chemistry", "affinity_gain"): rng.uniform(2.0, 12.0),
        ("chemistry", "maximum_efficiency"): rng.uniform(0.6, 0.95),
        ("chemistry", "uptake_capacity_per_second"): logu(rng, 1.0, 12.0),
        ("chemistry", "uptake_half_saturation_density"): logu(rng, 0.05, 1.5),
        ("chemistry", "motion_response_norm_limit"): logu(rng, 8.0, 256.0),
        ("chemistry", "maximum_acceleration"): logu(rng, 1.0, 40.0),
        ("chemistry", "drag_per_second"): logu(rng, 0.3, 6.0),
        ("chemistry", "maximum_speed"): logu(rng, 0.5, 12.0),
        ("sensors", "maximum_scale_world_units"): logu(rng, 2.0, 16.0),
        ("emission", "maximum_fractional_rate_per_second"): logu(rng, 0.05, 3.0),
        ("emission", "transfer_efficiency"): rng.uniform(0.6, 0.98),
        ("emission", "gate_threshold"): rng.uniform(0.1, 0.8),
        ("costs", "basal_energy_per_second"): logu(rng, 0.1, 2.0),
        ("costs", "motion_energy_per_speed_squared_second"): logu(rng, 0.0003, 0.02),
        ("reproduction", "minimum_parent_energy_before_attempt"): reserve,
        ("reproduction", "maximum_attempt_rate_per_second"): logu(rng, 0.3, 4.0),
        ("reproduction", "cooldown_seconds"): rng.uniform(0.05, 2.0),
        ("mutation", "weight_probability"): logu(rng, 0.001, 0.05),
        ("mutation", "weight_standard_deviation"): logu(rng, 0.01, 0.2),
        ("mutation", "chemistry_standard_deviation"): logu(rng, 0.003, 0.08),
        ("death", "residual_energy_recycle_fraction"): rng.uniform(0.0, 0.9),
        ("population", "initial_energy"): reserve * rng.uniform(0.3, 1.0),
        ("population", "initial_count"): int(cap * rng.uniform(0.3, 1.0)),
        ("population", "maximum_count"): cap,
        ("population", "capacity_policy"): "refuse-excess-births-v1",
    }


def sample_signal(index: int) -> dict:
    """A configuration from random.Random(400000 + index) where emission can shape the field:
    in the showcase world emission is 2.5% of the field's input, too little for the lineage
    signatures organisms emit (kin emission directions agree at cosine 0.75 to 0.95) to steer
    anyone. Here the signal stays local (low diffusion, faster decay), the taxis budget is
    large, most organisms' gates open, emission is cheap to deliver and the capped
    population holds energy to spend on it."""
    rng = random.Random(400000 + index)
    logu = base.logu
    area = base.SIDE * base.SIDE
    count = rng.choice([16, 32, 64, 128])
    supply = logu(rng, 0.3, 4.0)
    cap = int(logu(rng, 0.5, 3.0) * area)
    reserve = logu(rng, 2.0, 30.0)
    return {
        ("model", "model"): rng.choice(["field-ecology-v1", "field-ecology-v1-ungated-motion"]),
        ("world", "diffusion"): logu(rng, 0.02, 0.5),
        ("world", "decay_per_second"): logu(rng, 0.03, 1.0),
        ("sources", "count"): count,
        ("sources", "sigma_world_units"): logu(rng, 4.0, 30.0),
        ("sources", "power_per_source"): supply * area / count,
        ("sources", "path_speed"): logu(rng, 0.2, 6.0),
        ("chemistry", "toxicity_sensitivity"): logu(rng, 0.02, 1.0),
        ("chemistry", "discrimination_threshold"): rng.uniform(0.05, 0.4),
        ("chemistry", "affinity_gain"): rng.uniform(2.0, 12.0),
        ("chemistry", "maximum_efficiency"): rng.uniform(0.6, 0.95),
        ("chemistry", "uptake_capacity_per_second"): logu(rng, 1.0, 12.0),
        ("chemistry", "uptake_half_saturation_density"): logu(rng, 0.05, 1.5),
        ("chemistry", "motion_response_norm_limit"): logu(rng, 32.0, 512.0),
        ("chemistry", "maximum_acceleration"): logu(rng, 4.0, 60.0),
        ("chemistry", "drag_per_second"): logu(rng, 0.5, 6.0),
        ("chemistry", "maximum_speed"): logu(rng, 1.0, 12.0),
        ("sensors", "maximum_scale_world_units"): logu(rng, 2.0, 16.0),
        ("emission", "maximum_fractional_rate_per_second"): logu(rng, 0.3, 6.0),
        ("emission", "transfer_efficiency"): rng.uniform(0.85, 0.99),
        ("emission", "gate_threshold"): rng.uniform(0.02, 0.4),
        ("costs", "basal_energy_per_second"): logu(rng, 0.05, 1.0),
        ("costs", "motion_energy_per_speed_squared_second"): logu(rng, 0.0003, 0.01),
        ("reproduction", "minimum_parent_energy_before_attempt"): reserve,
        ("reproduction", "maximum_attempt_rate_per_second"): logu(rng, 0.3, 4.0),
        ("reproduction", "cooldown_seconds"): rng.uniform(0.05, 2.0),
        ("mutation", "weight_probability"): logu(rng, 0.001, 0.05),
        ("mutation", "weight_standard_deviation"): logu(rng, 0.01, 0.2),
        ("mutation", "chemistry_standard_deviation"): logu(rng, 0.003, 0.08),
        ("death", "residual_energy_recycle_fraction"): rng.uniform(0.0, 0.9),
        ("population", "initial_energy"): reserve * rng.uniform(0.5, 1.0),
        ("population", "initial_count"): int(cap * rng.uniform(0.5, 1.0)),
        ("population", "maximum_count"): cap,
        ("population", "capacity_policy"): "refuse-excess-births-v1",
    }


def score(work: Path, ref: dict) -> dict:
    """search.py's survival and motion, and the distance of the last third's frames."""
    result = base.score(work)
    if result.get("why") == "dying" or not result.get("frames"):
        result["distance"] = None
        return result
    records = [json.loads(line) for line in (work / "render.ndjson").read_text().splitlines() if line.strip()]
    frames = [r for r in records if r.get("record") == "frame"]
    late = frames[len(frames) * 2 // 3:]
    stats = np.mean([features(cells(work / "frames" / Path(f["path"]).name, base.SIDE)) for f in late], axis=0)
    result["distance"] = round(distance(stats, ref), 3)
    result["stats"] = [round(float(v), 4) for v in stats]
    return result


def start(index: int, parents=None, sampler=None):
    name = base.name_of(index)
    work = base.lab() / name
    work.mkdir(parents=True, exist_ok=True)
    if parents:
        parent, scale, params = base.local(index, parents)
        # search.py's `local` resets the cap for pause-before-commit; a refusing parent keeps
        # its own cap, perturbed like every other number.
        was = json.loads((base.lab() / parent / "params.json").read_text())
        if was.get("population.capacity_policy") == "refuse-excess-births-v1":
            cap = int(was["population.maximum_count"] * math.exp(random.Random(300000 + index).uniform(
                math.log(0.7), math.log(1.4))))
            params[("population", "maximum_count")] = cap
            params[("population", "initial_count")] = min(int(params[("population", "initial_count")]), cap)
        (work / "origin.json").write_text(json.dumps({"parent": parent, "source_scale": scale}))
    else:
        params = (sampler or sample_wide)(index)
    (work / "params.json").write_text(json.dumps({f"{s}.{k}": v for (s, k), v in params.items()}, indent=1))
    config = work / "config.toml"
    config.write_text(base.config_text(params, f"match-{name}"))
    valid = subprocess.run([base.flife(), "validate", str(config)], capture_output=True, text=True, timeout=300)
    if valid.returncode != 0:
        (work / "score.json").write_text(json.dumps({"ok": False, "why": "invalid", "detail": valid.stdout[-300:]}))
        return None
    flags = ["--render-threads", "1", "--controllers", "cpu", "--no-field", "--look", "raw", "--color", "affinity"]
    return base.render(config, work, base.TICKS, base.EVERY, base.PIXELS, base.WORKERS, flags)


def run(first: int, count: int, parallel: int, parents=None, sampler=None) -> None:
    ref = json.loads((HERE / "reference.json").read_text())
    pending = list(range(first, first + count))
    running = {}
    began = time.time()
    while pending or running:
        while pending and len(running) < parallel:
            index = pending.pop(0)
            if (base.lab() / base.name_of(index) / "score.json").exists():
                continue
            process = start(index, parents, sampler)
            if process is not None:
                running[index] = (process, time.time())
        for index, (process, t0) in list(running.items()):
            if process.poll() is None:
                if time.time() - t0 > 900:
                    process.kill()
                continue
            del running[index]
            work = base.lab() / base.name_of(index)
            try:
                result = score(work, ref)
                if process.returncode != 0:
                    detail = (work / "render.err").read_text()
                    result["ok"] = False
                    result["why"] = "paused" if "paused" in detail else f"exit {process.returncode}"
            except Exception as error:  # a scoring failure is recorded, not fatal
                result = {"ok": False, "why": f"score: {error}"}
            (work / "score.json").write_text(json.dumps(result))
            print(base.name_of(index), json.dumps({k: v for k, v in result.items() if k != "stats"}), flush=True)
        time.sleep(1)
    print(f"RUN DONE {first}..{first + count} in {time.time() - began:.0f}s", flush=True)


def rank(top: int) -> None:
    rows = []
    for work in sorted(base.lab().glob("s[0-9][0-9][0-9][0-9]")):
        path = work / "score.json"
        if not path.exists():
            continue
        result = json.loads(path.read_text())
        if result.get("distance") is None:
            continue
        rows.append((work.name, result))
    rows.sort(key=lambda row: row[1]["distance"])
    for name, r in rows[:top * 2]:
        print(name, f"distance {r['distance']:.3f} motion {r.get('motion', 0):.2f} live {r['live_initial']}->{r['live_end']}",
              r.get("why", ""))
    base.sheet(base.lab() / "match-top.png", [name for name, _ in rows[:top]])


if __name__ == "__main__":
    a = sys.argv
    command = a[1] if len(a) > 1 else "help"
    if command == "reference":
        reference(Path(a[2]), [Path(p) for p in a[3:]])
    elif command == "run":
        run(int(a[2]), int(a[3]), int(a[4]) if len(a) > 4 else 8)
    elif command == "signal":
        run(int(a[2]), int(a[3]), int(a[4]) if len(a) > 4 else 8, sampler=sample_signal)
    elif command == "local":
        run(int(a[3]), int(a[4]), int(a[5]) if len(a) > 5 else 8, parents=a[2].split(","))
    elif command == "score":
        print(json.dumps(score(base.lab() / a[2], json.loads((HERE / "reference.json").read_text()))))
    elif command == "rescore":
        ref = json.loads((HERE / "reference.json").read_text())
        for work in sorted(base.lab().glob("s[0-9][0-9][0-9][0-9]")):
            path = work / "score.json"
            if not path.exists() or json.loads(path.read_text()).get("why") == "invalid":
                continue
            old = json.loads(path.read_text())
            result = score(work, ref)
            # A render that exited early keeps the reason it was given.
            if old.get("ok") is False and old.get("why") not in (None, "dying"):
                result["ok"], result["why"] = False, old["why"]
            path.write_text(json.dumps(result))
            print(work.name, result.get("distance"), flush=True)
    elif command == "rank":
        rank(int(a[2]) if len(a) > 2 else 12)
    else:
        print(__doc__)

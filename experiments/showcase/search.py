"""The outer search that chose presets/showcase-world.toml (README.md beside this file).

    FLIFE=path/to/flife SHOWCASE_LAB=work/dir python3 search.py COMMAND ...

    run FIRST COUNT [PARALLEL]          stage 1: sample and run configurations FIRST..FIRST+COUNT
    local P1,P2,... FIRST COUNT [PAR]   stage 2: perturbations of the named samples
    score NAME                          (re)score one sample from its frames
    record                              rescore every sample and write scores.ndjson beside this file
    rank [N]                            print the ranking; contact sheet of the top N (top.png)
    held-out NAME SIDE SEED TICKS EVERY PIXELS WORKERS [render flags...]
                                        NAME on a SIDE x SIDE world with another seed
    sheet OUT.png NAME[:FRAME] ...      last (or given) frames of samples, four a row

`flife` must be built from this revision (cargo build --release -p flife-cli).
Frames and records go to SHOWCASE_LAB, outside the repository (AGENTS.md: no
large artifacts in git).

The search is outside the world: it reads a run's PNG frames and NDJSON
records after the run, and nothing it computes feeds back into any run.

Stage 1. Sample i draws its parameters from random.Random(i) (`sample`), so
a sample is reproducible from its index alone. The world is 128 x 128 world
units at dt = 1/32 s, seed 42, otherwise presets/desktop-world.toml; each
runs 1,600 ticks with a raw-dot frame (no field) every 40 ticks.

Stage 2. Sample i (from 1000) perturbs a parent drawn by random.Random(100000
+ i) (`local`): every number times a factor in [0.7, 1.4], and the source
count times 1, 2, 4 or 8 with each source's power divided by the same, so the
total supply is the parent's perturbed supply.

Scores, from the luminance of the second half of a sample's frames, one value
per world cell:
- fine: mean spatial power at wavelengths of 4-16 cells over the white level
  (the power at wavelengths up to 2.5 cells, where particle shot noise
  dominates): membranes, filaments, small cells;
- coarse: the same at 24-128 cells (blobs, domains);
- motion: 1 - the correlation of frames two apart, blurred two cells
  (structure moving and changing; 0 for a frozen world);
- coverage: the share of cells lit above a quarter of the mean once the
  background colour is removed and blurred two cells (how much of the world
  the organisms fill).
The ranking key is fine * min(1, motion / 0.3) * min(1, coverage / 0.5).
"""

import json
import math
import os
import random
import re
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.ndimage import gaussian_filter

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
SIDE = 128
TICKS = 1600
EVERY = 40
PIXELS = 384
WORKERS = 4


def flife() -> str:
    path = os.environ.get("FLIFE")
    if not path or not Path(path).is_file() or not os.access(path, os.X_OK):
        raise SystemExit("set FLIFE to a flife binary built from this revision")
    return str(Path(path).resolve())


def lab() -> Path:
    path = os.environ.get("SHOWCASE_LAB")
    if not path:
        raise SystemExit("set SHOWCASE_LAB to a work directory outside the repository")
    return Path(path).resolve()


def logu(rng, lo, hi):
    return math.exp(rng.uniform(math.log(lo), math.log(hi)))


def sample(index: int) -> dict:
    rng = random.Random(index)
    area = SIDE * SIDE
    count = rng.choice([2, 4, 8, 16, 32])
    supply = logu(rng, 0.2, 3.0)  # energy per cell per second, all sources together
    density = logu(rng, 0.15, 0.8)
    initial = int(density * area)
    return {
        ("model", "model"): rng.choice(["field-ecology-v1", "field-ecology-v1-ungated-motion"]),
        ("world", "diffusion"): logu(rng, 0.03, 1.5),
        ("world", "decay_per_second"): logu(rng, 0.003, 0.3),
        ("sources", "count"): count,
        ("sources", "sigma_world_units"): logu(rng, 2.0, 24.0),
        ("sources", "power_per_source"): supply * area / count,
        ("sources", "path_speed"): logu(rng, 0.2, 8.0),
        ("chemistry", "toxicity_sensitivity"): logu(rng, 0.05, 2.0),
        ("chemistry", "discrimination_threshold"): rng.uniform(0.02, 0.4),
        ("chemistry", "affinity_gain"): rng.uniform(2.0, 10.0),
        ("chemistry", "maximum_efficiency"): rng.uniform(0.6, 0.95),
        ("chemistry", "uptake_capacity_per_second"): logu(rng, 2.0, 16.0),
        ("chemistry", "uptake_half_saturation_density"): logu(rng, 0.02, 1.0),
        ("chemistry", "motion_response_norm_limit"): logu(rng, 8.0, 512.0),
        ("chemistry", "maximum_acceleration"): logu(rng, 2.0, 40.0),
        ("chemistry", "drag_per_second"): logu(rng, 0.3, 6.0),
        ("chemistry", "maximum_speed"): rng.uniform(2.0, 15.0),
        ("emission", "maximum_fractional_rate_per_second"): logu(rng, 0.05, 2.0),
        ("emission", "transfer_efficiency"): rng.uniform(0.7, 0.98),
        ("emission", "gate_threshold"): rng.uniform(0.2, 0.8),
        ("costs", "basal_energy_per_second"): logu(rng, 0.2, 2.0),
        ("costs", "motion_energy_per_speed_squared_second"): logu(rng, 0.0005, 0.02),
        ("reproduction", "minimum_parent_energy_before_attempt"): rng.uniform(1.5, 4.0),
        ("reproduction", "maximum_attempt_rate_per_second"): logu(rng, 0.3, 4.0),
        ("reproduction", "cooldown_seconds"): rng.uniform(0.1, 1.0),
        ("death", "residual_energy_recycle_fraction"): rng.uniform(0.0, 0.9),
        ("population", "initial_count"): initial,
        ("population", "maximum_count"): min(initial * 8, 48000),
    }


NUMERIC_KEEP = {("population", "maximum_count"), ("sources", "count")}


def local(index: int, parents: list) -> tuple:
    """A perturbation of one parent: every number times a factor in [0.7, 1.4]
    (bounded where the validator bounds it), the source count times 1, 2, 4 or
    8 with each source's power divided by the same, and the model kept."""
    rng = random.Random(100000 + index)
    parent = rng.choice(parents)
    base = json.loads((lab() / parent / "params.json").read_text())
    out = {}
    for flat, value in base.items():
        section, key = flat.split(".", 1)
        out[(section, key)] = value
    scale = rng.choice([1, 2, 4, 8])
    for k, value in list(out.items()):
        if isinstance(value, str) or k in NUMERIC_KEEP or k == ("population", "initial_count"):
            continue
        out[k] = value * math.exp(rng.uniform(math.log(0.7), math.log(1.4)))
    out[("sources", "count")] = int(base["sources.count"]) * scale
    out[("sources", "power_per_source")] = out[("sources", "power_per_source")] / scale
    bounded = {
        ("chemistry", "maximum_efficiency"): 0.97,
        ("emission", "transfer_efficiency"): 0.99,
        ("emission", "gate_threshold"): 0.95,
        ("death", "residual_energy_recycle_fraction"): 0.95,
        ("chemistry", "discrimination_threshold"): 0.9,
        ("chemistry", "maximum_speed"): 15.5,
    }
    for k, top in bounded.items():
        out[k] = min(out[k], top)
    out[("population", "initial_count")] = int(base["population.initial_count"])
    out[("population", "maximum_count")] = 48000
    return parent, scale, out


def literal(value):
    if isinstance(value, str):
        return f'"{value}"'
    if isinstance(value, int):
        return str(value)
    return repr(float(f"{value:.6g}"))


def config_text(changes: dict, name: str, side: int = SIDE, seed: int = 42) -> str:
    """presets/desktop-world.toml with `changes`, on a `side` x `side` world at dt 1/32 s."""
    text = (REPO / "presets" / "desktop-world.toml").read_text()
    fixed = {
        ("world", "width"): f"{side}.0",
        ("world", "height"): f"{side}.0",
        ("world", "grid_width"): str(side),
        ("world", "grid_height"): str(side),
        ("world", "tick_denominator"): "32",
    }
    left = {key: literal(v) for key, v in changes.items() if key != ("model", "model")}
    left.update(fixed)
    # A contact model's [contact] table (plan §5.4) is not in the preset: it is appended.
    contact = {key: left.pop(("contact", key)) for key in
               ["diameter", "stiffness", "adhesion", "adhesion_range"] if ("contact", key) in left}
    section = None
    out = []
    for line in text.split("\n"):
        header = re.match(r"^\[([a-z_]+)\]\s*$", line)
        if header:
            section = header.group(1)
        key = re.match(r"^([a-z_]+)\s*=", line)
        if key and (section, key.group(1)) in left:
            line = f"{key.group(1)} = {left.pop((section, key.group(1)))}"
        out.append(line)
    if left:
        raise SystemExit(f"keys not found: {sorted(left)}")
    text = "\n".join(out)
    for old, new in [
        ('name = "desktop-world"', f'name = "{name}"'),
        ('model = "field-ecology-v1"', f'model = "{changes[("model", "model")]}"'),
        ("\nseed = 42\n", f"\nseed = {seed}\n"),
    ]:
        assert text.count(old) == 1, old
        text = text.replace(old, new, 1)
    if contact:
        text = text.rstrip("\n") + "\n\n[contact]\n" + "".join(f"{k} = {v}\n" for k, v in contact.items())
    return text


def luminance(path: Path, side: int = SIDE) -> np.ndarray:
    image = np.asarray(Image.open(path).convert("RGB"), dtype=np.float64)
    grey = image.mean(axis=2)
    # One value per world cell: sum each (pixels / side)^2 block.
    k = grey.shape[0] // side
    return grey.reshape(side, k, side, k).sum(axis=(1, 3))


def band_power(field: np.ndarray, lo: float, hi: float) -> float:
    """Mean power at wavelengths [lo, hi] cells over the white (shot-noise) level."""
    mean = field.mean()
    if mean <= 0:
        return 0.0
    f = np.fft.fft2(field - mean)
    power = (np.abs(f) ** 2) / field.size
    ky = np.fft.fftfreq(field.shape[0])[:, None]
    kx = np.fft.fftfreq(field.shape[1])[None, :]
    k = np.sqrt(kx * kx + ky * ky)
    with np.errstate(divide="ignore"):
        wavelength = np.where(k > 0, 1.0 / k, np.inf)
    band = (wavelength >= lo) & (wavelength <= hi)
    white = power[(wavelength <= 2.5)].mean()
    if white <= 0:
        return 0.0
    return float(power[band].mean() / white)


def score(work: Path, side: int = SIDE) -> dict:
    records = [json.loads(line) for line in (work / "render.ndjson").read_text().splitlines() if line.strip()]
    frames = [r for r in records if r.get("record") == "frame"]
    end = next((r for r in records if r.get("record") == "end"), None)
    if not frames:
        return {"ok": False, "why": "no frames"}
    lives = [f["live"] for f in frames]
    initial = lives[0]
    late = frames[len(frames) // 2:]
    result = {
        "ok": end is not None,
        "live_end": lives[-1],
        "live_min_late": min(f["live"] for f in late),
        "live_initial": initial,
        "speed": round(float(np.mean([f["mean_speed"] for f in late])), 3),
        "frames": len(frames),
        "last_tick": frames[-1]["tick"],
    }
    if lives[-1] < 0.1 * initial or lives[-1] < 200:
        result.update(fine=0.0, coarse=0.0, motion=0.0, coverage=0.0, why="dying")
        return result
    # Frames are read from the work directory, wherever it has moved.
    fields = [luminance(work / "frames" / Path(f["path"]).name, side) for f in late]
    result["fine"] = round(float(np.mean([band_power(d, 4, 16) for d in fields])), 3)
    covered = []
    for d in fields:
        lit = np.clip(d - np.percentile(d, 5), 0, None)
        blurred = gaussian_filter(lit, 2, mode="wrap")
        covered.append(float((blurred > 0.25 * blurred.mean()).mean()))
    result["coverage"] = round(float(np.mean(covered)), 3)
    result["coarse"] = round(float(np.mean([band_power(d, 24, 128) for d in fields])), 3)
    motions = []
    for a, b in zip(fields, fields[2:]):
        sa, sb = gaussian_filter(a, 2, mode="wrap"), gaussian_filter(b, 2, mode="wrap")
        sa, sb = sa - sa.mean(), sb - sb.mean()
        denominator = math.sqrt((sa * sa).sum() * (sb * sb).sum())
        motions.append(1.0 - (sa * sb).sum() / denominator if denominator > 0 else 0.0)
    result["motion"] = round(float(np.mean(motions)) if motions else 0.0, 3)
    return result


def key(result: dict) -> float:
    return (result.get("fine") or 0.0) * min(1.0, (result.get("motion") or 0.0) / 0.3) * min(
        1.0, (result.get("coverage") or 0.0) / 0.5)


def name_of(index: int) -> str:
    return f"s{index:04d}"


def render(config: Path, work: Path, ticks: int, every: int, pixels: int, workers: int, flags) -> subprocess.Popen:
    argv = [flife(), "render", str(config), "--ticks", str(ticks), "--every", str(every),
            "--width", str(pixels), "--height", str(pixels), "--cpu-workers", str(workers),
            *flags, "--out", str(work / "frames")]
    # The child keeps its own copies of the descriptors; the parent's close here.
    with open(work / "render.ndjson", "w") as out, open(work / "render.err", "w") as err:
        return subprocess.Popen(argv, stdout=out, stderr=err)


def start(index: int, parents=None):
    name = name_of(index)
    work = lab() / name
    work.mkdir(parents=True, exist_ok=True)
    if parents:
        parent, scale, params = local(index, parents)
        (work / "origin.json").write_text(json.dumps({"parent": parent, "source_scale": scale}))
    else:
        params = sample(index)
    (work / "params.json").write_text(json.dumps({f"{s}.{k}": v for (s, k), v in params.items()}, indent=1))
    config = work / "config.toml"
    config.write_text(config_text(params, f"search-{name}"))
    valid = subprocess.run([flife(), "validate", str(config)], capture_output=True, text=True)
    if valid.returncode != 0:
        (work / "score.json").write_text(json.dumps({"ok": False, "why": "invalid", "detail": valid.stdout[-300:]}))
        return None
    flags = ["--render-threads", "1", "--controllers", "cpu", "--no-field", "--look", "raw"]
    return render(config, work, TICKS, EVERY, PIXELS, WORKERS, flags)


def run(first: int, count: int, parallel: int, parents=None) -> None:
    pending = list(range(first, first + count))
    running = {}
    began = time.time()
    while pending or running:
        while pending and len(running) < parallel:
            index = pending.pop(0)
            if (lab() / name_of(index) / "score.json").exists():
                continue
            process = start(index, parents)
            if process is not None:
                running[index] = (process, time.time())
        for index, (process, t0) in list(running.items()):
            if process.poll() is None:
                if time.time() - t0 > 600:
                    process.kill()
                continue
            del running[index]
            work = lab() / name_of(index)
            try:
                result = score(work)
                if process.returncode != 0:
                    detail = (work / "render.err").read_text()
                    result["ok"] = False
                    # pause-before-commit: the population reached its ceiling.
                    result["why"] = "paused" if "paused" in detail else f"exit {process.returncode}"
            except Exception as error:  # a scoring failure is recorded, not fatal
                result = {"ok": False, "why": f"score: {error}"}
            (work / "score.json").write_text(json.dumps(result))
            print(name_of(index), json.dumps(result), flush=True)
        time.sleep(1)
    print(f"RUN DONE {first}..{first + count} in {time.time() - began:.0f}s", flush=True)


def record() -> None:
    """Rescore every sample with `score` and write one line each to scores.ndjson."""
    lines = []
    for work in sorted(lab().glob("s[0-9][0-9][0-9][0-9]")):
        stored = json.loads((work / "score.json").read_text()) if (work / "score.json").exists() else None
        if stored is None:
            row = {"sample": work.name, "ok": False, "why": "interrupted: no score"}
        elif stored.get("why") == "invalid":
            row = {"sample": work.name, **stored}
        else:
            row = {"sample": work.name, **score(work)}
            if stored.get("why") and not row.get("why"):
                row.update(ok=False, why=stored["why"])
        params = json.loads((work / "params.json").read_text())
        row["model"] = params["model.model"]
        row["sources"] = params["sources.count"]
        if (work / "origin.json").exists():
            row.update(json.loads((work / "origin.json").read_text()))
        row["key"] = round(key(row), 3)
        lines.append(json.dumps(row, sort_keys=True))
    (HERE / "scores.ndjson").write_text("\n".join(lines) + "\n")
    print(f"{len(lines)} samples -> {HERE / 'scores.ndjson'}")


def rank(top: int) -> None:
    rows = [json.loads(line) for line in (HERE / "scores.ndjson").read_text().splitlines()]
    rows = [r for r in rows if r.get("fine") is not None]
    rows.sort(key=lambda r: r["key"], reverse=True)
    for r in rows[:top * 2]:
        print(r["sample"], f"key {r['key']:7.2f} fine {r['fine']:7.2f} motion {r['motion']:.3f} "
              f"cover {r['coverage']:.2f} live {r['live_initial']}->{r['live_end']} {r.get('why', '')}")
    sheet(lab() / "top.png", [r["sample"] for r in rows[:top]])


def held_out(name: str, side: int, seed: int, ticks: int, every: int, pixels: int, workers: int, flags) -> int:
    """Sample NAME on a SIDE x SIDE world with SEED: the source count and founders
    scale with the area, so source density, supply per cell and organism density
    are the sample's."""
    base = json.loads((lab() / name / "params.json").read_text())
    params = {tuple(flat.split(".", 1)): value for flat, value in base.items()}
    factor = (side / SIDE) ** 2
    params[("sources", "count")] = int(round(base["sources.count"] * factor))
    initial = int(base["population.initial_count"] * factor)
    params[("population", "initial_count")] = initial
    cap = min(initial * 8, 400000)
    params[("population", "maximum_count")] = cap
    genome_bytes = cap * 12000 * 4
    params[("execution", "maximum_logical_genome_bytes")] = genome_bytes
    params[("execution", "maximum_allocated_genome_bytes")] = int(genome_bytes * 1.2)
    params[("execution", "maximum_total_bytes")] = int(genome_bytes * 3)
    work = lab() / "held-out" / f"{name}-{side}-seed{seed}"
    work.mkdir(parents=True, exist_ok=True)
    config = work / "config.toml"
    config.write_text(config_text(params, f"held-out-{name}-{side}-seed{seed}", side, seed))
    valid = subprocess.run([flife(), "validate", str(config)], capture_output=True, text=True)
    if valid.returncode != 0:
        raise SystemExit(f"invalid: {valid.stdout[-600:]}")
    return render(config, work, ticks, every, pixels, workers, flags).wait()


def sheet(out: Path, picks) -> None:
    inputs = []
    for pick in picks:
        name, _, frame = pick.partition(":")
        frames = sorted((lab() / name / "frames").glob("frame-*.png"))
        path = frames[int(frame)] if frame else frames[-1]
        inputs += ["-i", str(path)]
    n = len(picks)
    if n == 0:
        return
    layout = "|".join(f"{(i % 4) * 384}_{(i // 4) * 384}" for i in range(n))
    scaled = "".join(f"[{i}]scale=384:384[v{i}];" for i in range(n))
    stacked = "".join(f"[v{i}]" for i in range(n))
    graph = scaled + stacked + f"xstack=inputs={n}:layout={layout}:fill=black" if n > 1 else "[0]scale=384:384"
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", *inputs, "-filter_complex", graph, str(out)], check=True)
    print("sheet", out)


if __name__ == "__main__":
    a = sys.argv
    command = a[1] if len(a) > 1 else "help"
    if command == "run":
        run(int(a[2]), int(a[3]), int(a[4]) if len(a) > 4 else 8)
    elif command == "local":
        run(int(a[3]), int(a[4]), int(a[5]) if len(a) > 5 else 8, parents=a[2].split(","))
    elif command == "score":
        print(json.dumps(score(lab() / a[2])))
    elif command == "record":
        record()
    elif command == "rank":
        rank(int(a[2]) if len(a) > 2 else 12)
    elif command == "held-out":
        code = held_out(a[2], int(a[3]), int(a[4]), int(a[5]), int(a[6]), int(a[7]), int(a[8]), a[9:])
        print("exit", code)
    elif command == "sheet":
        sheet(Path(a[2]), a[3:])
    else:
        print(__doc__)

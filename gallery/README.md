# FrankenLife Provenance-Backed Visual Gallery (FL-049)

> **Status:** **Observed (`5ee847f`, 2026-11-27)** — compact gallery manifest (`gallery/manifest.json`) and camera paths (`gallery/cameras/`) covering all six visual categories required by plan §2.4 (Gates G5 and G9). Large PNG/MP4 media remain in artifact storage rather than Git history (plan §18.5).

## Four-Claims Discipline

Per `AGENTS.md` and plan §15 / §21:
- **Visual claim only:** Every recording here is genuine `flife render` output paired with an identical-world **raw Gate G5 view** (`--look raw`: unblended dots, zero bloom, linear greyscale field) verifying `INV-09` presentation purity (identical SHA-256 `World::digest()` between aesthetic and raw views).
- **Ecological claim stays separate:** Evolutionary and morphological claims are supported solely by the held-out multi-seed experiments in `experiments/fl048/README.md`, `experiments/showcase/README.md`, and `experiments/v2-emergence/README.md` (with failed and extinct runs retained).

## The Six Qualified Categories (plan §2.4)

| # | Category | Preset | Model | Tick | Aesthetic View | Paired Raw G5 View | Verified World SHA-256 Digest (12 hex) |
|---|---|---|---|---:|---|---|---|
| 1 | `dense-world` | `presets/confluent-world.toml` | `field-ecology-v1-contact` | `64` | `sphere` / `lineage-energy` | `raw` / `lineage` | `31c7038468ed` |
| 2 | `deep-zoom` | `presets/tissue-world.toml` | `field-ecology-v1` | `64` | `glow` / `lineage` (`4×` zoom) | `raw` / `lineage` (`4×` zoom) | `554d8588771b` |
| 3 | `interacting-boundary` | `presets/colony-world.toml` | `field-ecology-v1` | `64` | `sphere` / `affinity` + `composition` field | `raw` / `affinity` + `energy` field | `caa68b6531bb` |
| 4 | `perturbed-colony` | `presets/showcase-world.toml` | `field-ecology-v1` | `96` | `glow` / `energy` (inject @16 + recycle-remove @32) | `raw` / `energy` (from same checkpointed `--run-dir`) | `6b7182ff4ec2` |
| 5 | `lineage-view` | `presets/morphogenesis-tissue-v2.toml` | `field-ecology-v2` | `64` | `sphere` / `cell-state` | `raw` / `lineage` | `24030abcb783` |
| 6 | `raw-field-view` | `presets/supercolony-v2.toml` | `field-ecology-v2` | `64` | `glow` / `affinity` + `trail-carrier` | `raw` / `energy` + linear `energy` field | `fbe486653489` |

## Verification

Run the automated manifest integrity, preset-hash, camera-path, and live `INV-09` aesthetic-vs-raw render parity test:

```bash
cargo test --locked -p flife-cli --test gallery_manifest
```

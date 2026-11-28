#!/usr/bin/env bash
# Does a flife build keep FL-034's two evolved worlds (README.md, "FL-034 G4")? Usage:
#
#   experiments/fl034/check-evolved-worlds.sh FLIFE OUT_DIR [CONTROLLERS] [WORKERS]
#
# FLIFE is a release flife binary; OUT_DIR a new or empty directory for the runs' records. CONTROLLERS is metal
# (default on macOS) or cpu (default elsewhere); WORKERS defaults to 16. Each world runs 8,192 ticks and is compared
# with the G4 rows checkpoint by checkpoint (live, births, deaths) and by its end digest. It prints EQUAL, or the
# first checkpoint where the world parts. Exit 0 when both are equal, 1 when either parts, 2 on usage errors.
# On an M5 Ultra under load the Interactive world takes about 4 minutes and the Desktop world about 13.
set -u
FLIFE=${1:?usage: check-evolved-worlds.sh FLIFE OUT_DIR [CONTROLLERS] [WORKERS]}
OUT=${2:?usage: check-evolved-worlds.sh FLIFE OUT_DIR [CONTROLLERS] [WORKERS]}
if [[ "$(uname -s)" == Darwin ]]; then CONTROLLERS=${3:-metal}; else CONTROLLERS=${3:-cpu}; fi
WORKERS=${4:-16}
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
[[ -x "$FLIFE" ]] || { echo "not an executable: $FLIFE" >&2; exit 2; }
mkdir -p "$OUT" || exit 2
if [[ -n "$(ls -A "$OUT")" ]]; then echo "refusing: $OUT is not empty" >&2; exit 2; fi

# The Desktop world is presets/desktop-world.toml on E2's held-out seed 2022.
sed 's/^seed = 42$/seed = 2022/' "$ROOT/presets/desktop-world.toml" > "$OUT/desktop-2022.toml"
grep -q '^seed = 2022$' "$OUT/desktop-2022.toml" || { echo "could not set the Desktop seed" >&2; exit 2; }

if command -v shasum > /dev/null; then shasum -a 256 "$FLIFE"; else sha256sum "$FLIFE"; fi
echo "controllers $CONTROLLERS, $WORKERS workers"

# tick:live:births:deaths at each checkpoint of the G4 rows, then the end digest.
INTERACTIVE_REF="1024:6253:3922:22669 2048:25904:26621:25717 3072:27327:46357:44030 4096:22354:55692:58338
5120:22326:65020:67694 6144:22171:73753:76582 7168:22479:82607:85128 8192:22559:92229:94670"
INTERACTIVE_DIGEST=595e2d90bcdbaa10db0738d78387f9e88ae63459997f33525449ee117053167b
DESKTOP_REF="256:88445:442:11997 512:15266:2766:87500 768:18370:7669:89299 1024:26534:16515:89981
1280:40378:31013:90635 1536:60531:52441:91910 1792:84137:79283:95146 2048:104622:107951:103329
2304:116506:134585:118079 2560:119110:156796:137686 2816:114577:174252:159675 3072:108011:188089:180078
3328:101573:199275:197702 3584:96524:209316:212792 3840:93329:218886:225557 4096:91947:228665:236718
4352:91667:238951:247284 4608:92465:249669:257204 4864:93425:260359:266934 5120:94716:271157:276441
5376:95931:281731:285800 5632:97095:292125:295030 5888:97546:301740:304194 6144:97788:311120:313332
6400:97684:320147:322463 6656:97564:329167:331603 6912:97655:338380:340725 7168:97820:347927:350107
7424:98278:357922:359644 7680:98551:368073:369522 7936:98870:378450:379580 8192:99065:389045:389980"
DESKTOP_DIGEST=4f7f1cf19d484b6297bc1d56cfbc7dd96207f6b441dc6cae82dd401896e7e8c9

check() {  # name config checkpoint-interval reference digest
  local name=$1 config=$2 every=$3 ref=$4 digest=$5
  local record="$OUT/$name.ndjson"
  echo "== $name start $(date +%H:%M:%S)"
  "$FLIFE" run "$config" --ticks 8192 --every "$every" --cpu-workers "$WORKERS" --controllers "$CONTROLLERS" \
    > "$record" 2>&1
  local code=$?
  echo "   exit $code end $(date +%H:%M:%S)"
  python3 -I - "$record" "$ref" "$digest" <<'PY'
import json, sys
record, ref, digest = sys.argv[1], sys.argv[2].split(), sys.argv[3]
rows = [json.loads(line) for line in open(record) if line.startswith("{")]
seen = {f"{r['tick']}": f"{r['tick']}:{r['live']}:{r['tally']['births']}:{r['tally']['deaths']}"
        for r in rows if r.get("record") == "progress"}
for want in ref:
    got = seen.get(want.split(":")[0])
    if got != want:
        print(f"   PARTS by checkpoint {want.split(':')[0]}: expected {want}, got {got}")
        sys.exit(1)
end = rows[-1] if rows else {}
if end.get("record") != "end" or end.get("digest") != digest:
    print(f"   PARTS at the end: expected {digest}, got {end.get('digest')}")
    sys.exit(1)
print(f"   EQUAL: {len(ref)} checkpoints and digest {digest[:8]}...{digest[-6:]}")
PY
}

status=0
check interactive "$ROOT/presets/interactive-world.toml" 1024 "$INTERACTIVE_REF" "$INTERACTIVE_DIGEST" || status=1
check desktop "$OUT/desktop-2022.toml" 256 "$DESKTOP_REF" "$DESKTOP_DIGEST" || status=1
exit $status

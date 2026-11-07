#!/usr/bin/env bash
# Run serially, with no builds or other benchmarks during capture.
set -euo pipefail
if [[ $# -lt 2 ]]; then
  echo 'usage: record-native-perf.sh NOTE OUTPUT [page|window|scroll] [seconds] [binary]' >&2
  exit 2
fi
repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
note="$1"
output="$2"
mode="${3:-window}"
seconds="${4:-10}"
binary="${5:-$repo_dir/target/release/carnet}"
mkdir -p "$output"
output="$(cd "$output" && pwd)"
trace="${output}.trace"
if [[ -e "$trace" || -e "$output/metadata.json" ]]; then
  echo "Output already exists: $output" >&2
  exit 2
fi
limit="$(python3 -c 'import sys; print(2*float(sys.argv[1])+10)' "$seconds")s"
knobs=(record --template "${CARNET_METAL_TEMPLATE:-Metal System Trace}")
if [[ "${CARNET_PRESENTATION_ONLY:-0}" == 1 ]]; then
  knobs=(record --instrument Display)
fi
if [[ "${CARNET_GPU_DISPLAY_ONLY:-0}" == 1 ]]; then
  knobs=(record --instrument GPU --instrument Display)
fi
for knob in KUI_FRAME_LATENCY KUI_FRAME_PACING KUI_WINDOW_AT KUI_SPECIALIZE KUI_ACQUIRE_THREAD CARNET_PERF_WINDOW; do
  if setting="$(printenv "$knob")"; then knobs+=(--env "$knob=$setting"); fi
done
python3 - "$binary" "$note" "$output" "${knobs[@]}" <<'PY'
import hashlib, json, sys
from pathlib import Path
binary, note, output = map(Path, sys.argv[1:4])
recording = dict(binary=str(binary.resolve()),
                 binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                 source_sha256=hashlib.sha256(note.read_bytes()).hexdigest(),
                 xctrace_options=sys.argv[4:])
(output / "recording.json").write_text(json.dumps(recording, indent=2) + "\n")
PY
system_profiler SPDisplaysDataType > "$output/displays.txt"
pmset -g batt > "$output/power.txt"
xcrun xctrace "${knobs[@]}" --output "$trace" --time-limit "$limit" --target-stdout "$output/app.log" --launch -- "$binary" \
  --native-perf "$note" "$output" "$mode" "$seconds" > "$output/xctrace.log" 2>&1
if [[ ! -f "$output/metadata.json" ]]; then cat "$output/app.log" >&2; exit 1; fi
xcrun xctrace export --input "$trace" --toc --output "$output/metal-toc.xml"
xcrun xctrace export --input "$trace" \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals" or @schema="displayed-surfaces-interval" or @schema="display-vsyncs-interval"]' \
  --output "$output/metal-data.xml"
python3 "$repo_dir/scripts/analyze-native-perf.py" "$output" --single-case

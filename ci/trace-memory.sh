#!/usr/bin/env bash
# Peak memory and file size of a long trace (roadmap M3, ADR-0021): a toy
# line of 1,000 components runs <events> events while its trace streams to
# disk; then the trace is read and replayed. Each mode runs <runs> times;
# the table shows the median, with min and max (ADR-0010: absolute numbers
# come from the CI proxy runner, median of at least 10 runs). Needs Linux:
# peak memory is the process's VmHWM.
# Usage: ci/trace-memory.sh [events] [runs]   (defaults: 10000000 10)
set -euo pipefail
cd "$(dirname "$0")/.."
events=${1:-10000000}
runs=${2:-10}

cargo build --release --locked -p taktsim-trace --example trace_memory >&2
bin=target/release/examples/trace_memory
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
trace=$dir/line.trace

field() { grep -o "$1=[^ ]*" "$dir/out" | cut -d= -f2; }

# Runs the example <runs> times; prints "<peak KiB> <seconds>" per run and
# keeps the last run's output in $dir/out. Every run must print the same
# output apart from its memory and wall time.
measure() {
  local first=""
  for _ in $(seq "$runs"); do
    "$bin" "$@" >"$dir/out"
    echo "$(field peak_rss_kib) $(field wall_seconds)"
    local out
    out=$(sed 's/ peak_rss_kib=.*//' "$dir/out")
    if [ -z "$first" ]; then
      first=$out
    elif [ "$out" != "$first" ]; then
      echo "error: runs differ: '$first' vs '$out'" >&2
      exit 1
    fi
  done
}

median() { awk '{ a[NR] = $1 } END { print (NR % 2) ? a[(NR + 1) / 2] : (a[NR / 2] + a[NR / 2 + 1]) / 2 }'; }
mib() { awk '{ printf "%.1f", $1 / 1024 }'; }

# Reads "<KiB> <seconds>" lines; prints a table row for <mode>: median, min
# and max peak memory, median wall time.
row() {
  local data kib sec
  data=$(cat)
  kib=$(cut -d' ' -f1 <<<"$data" | sort -n)
  sec=$(cut -d' ' -f2 <<<"$data" | sort -n)
  printf '| %s | %s | %s | %s | %s |\n' "$1" \
    "$(median <<<"$kib" | mib)" "$(head -n1 <<<"$kib" | mib)" "$(tail -n1 <<<"$kib" | mib)" \
    "$(median <<<"$sec")"
}

echo "### Trace memory: $events events, $runs runs per mode"
echo
echo "Runner: $(uname -sm), $(nproc) CPUs, $(awk '/MemTotal/ {printf "%.1f GiB", $2 / 1048576}' /proc/meminfo); commit $(git rev-parse --short HEAD)"
echo
echo "| mode | median peak RSS (MiB) | min | max | median wall time (s) |"
echo "| --- | --- | --- | --- | --- |"
measure notrace "$events" | row "run, no trace (baseline)"
measure write "$events" "$trace" | row "run, trace written"
written=$(cat "$dir/out")
measure read "$trace" | row "read every record"
read_hash=$(field stream_sha256)
measure replay "$trace" | row "replay"
echo
out=$written
bytes=$(grep -o 'bytes=[^ ]*' <<<"$out" | cut -d= -f2)
records=$(grep -o 'records=[^ ]*' <<<"$out" | cut -d= -f2)
hash=$(grep -o 'stream_sha256=[^ ]*' <<<"$out" | cut -d= -f2)
echo "Trace: $(grep -o 'components=[^ ]*' <<<"$out" | cut -d= -f2) components, $records records, $(grep -o 'chunks=[^ ]*' <<<"$out" | cut -d= -f2) chunks, $bytes bytes ($(awk -v b="$bytes" 'BEGIN {printf "%.1f MiB, %.1f bytes per record", b / 1048576, b / '"$records"'}')), stream SHA-256 $hash"
if [ "$read_hash" != "$hash" ]; then
  echo "error: the reader's stream hash $read_hash differs from the writer's" >&2
  exit 1
fi

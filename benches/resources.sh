#!/usr/bin/env bash
# Memory and CPU cost of a running router, summed across its whole process tree.
#
# The tree is the point. nullrouter runs eight services; 9Router runs one Node process. A
# figure taken from whichever process looks like "the router" would flatter nullrouter by a
# factor of eight, and that is the easiest way to build an unfair benchmark by accident.
# This sums every process it is given and prints the count, so a reader can see it summed
# the right number of things.
#
#   benches/resources.sh --label nullrouter --ports 20128,20129,20130,20131,20132,20133,20134,20135
#   benches/resources.sh --label 9router --ports 20127
#
# Run it while load is being applied, from a second shell. Idle numbers are not the
# interesting ones -- a router that is doing nothing costs nothing.
#
# Reports:
#   * steady-state RSS (median of samples) and peak RSS
#   * CPU-seconds consumed over the sampling window
#   * per-process breakdown, so a single leaky service is visible rather than averaged away

set -euo pipefail

LABEL=""
PORTS=""
SECONDS_TO_SAMPLE=30
INTERVAL=0.1
OUT_DIR="benches/results"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --label)    LABEL="$2"; shift 2 ;;
    --ports)    PORTS="$2"; shift 2 ;;
    --seconds)  SECONDS_TO_SAMPLE="$2"; shift 2 ;;
    --interval) INTERVAL="$2"; shift 2 ;;
    --out)      OUT_DIR="$2"; shift 2 ;;
    -h|--help)  sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac
done

[[ -n "$LABEL" ]] || { echo "--label is required" >&2; exit 2; }
[[ -n "$PORTS" ]] || { echo "--ports is required (comma-separated)" >&2; exit 2; }

mkdir -p "$OUT_DIR"
RESULT="$OUT_DIR/$(date -u +%Y%m%dT%H%M%SZ)-$LABEL-resources.txt"

# ── find the processes behind those ports ────────────────────────────────────
declare -a PIDS=()
declare -A PID_NAME=()
IFS=',' read -ra PORT_LIST <<< "$PORTS"
for port in "${PORT_LIST[@]}"; do
  pid="$(ss -ltnp 2>/dev/null | grep ":$port " | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | head -1)"
  if [[ -z "$pid" ]]; then
    echo "nothing listening on :$port" >&2
    continue
  fi
  # A process may hold several ports (one service, several listeners); count it once.
  if [[ -z "${PID_NAME[$pid]:-}" ]]; then
    PIDS+=("$pid")
    PID_NAME["$pid"]="$(tr -d '\0' < "/proc/$pid/comm" 2>/dev/null || echo unknown)"
  fi
done

# Include children: Node spawns workers, and pxpipe runs a Node worker of its own that
# belongs to nullrouter's cost whether or not it holds a port.
declare -a ALL=()
for pid in "${PIDS[@]}"; do
  ALL+=("$pid")
  while read -r child; do
    [[ -n "$child" ]] || continue
    ALL+=("$child")
    PID_NAME["$child"]="$(tr -d '\0' < "/proc/$child/comm" 2>/dev/null || echo child)"
  done < <(pgrep -P "$pid" 2>/dev/null || true)
done

[[ ${#ALL[@]} -gt 0 ]] || { echo "no processes found for ports $PORTS" >&2; exit 3; }

CLK_TCK="$(getconf CLK_TCK)"

cpu_ticks() { # utime + stime for one pid, 0 if it is gone
  local pid="$1"
  awk '{print $14 + $15}' "/proc/$pid/stat" 2>/dev/null || echo 0
}
rss_kb() {
  local pid="$1"
  awk '/^VmRSS:/ {print $2}' "/proc/$pid/status" 2>/dev/null || echo 0
}

{
  echo "label: $LABEL"
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "ports: $PORTS"
  echo "processes: ${#ALL[@]}"
  for pid in "${ALL[@]}"; do
    echo "  $pid ${PID_NAME[$pid]:-?}"
  done
  echo "window: ${SECONDS_TO_SAMPLE}s at ${INTERVAL}s"
  echo
} | tee "$RESULT"

# ── sample ───────────────────────────────────────────────────────────────────
declare -A START_TICKS=()
for pid in "${ALL[@]}"; do START_TICKS["$pid"]="$(cpu_ticks "$pid")"; done

TOTALS_FILE="$(mktemp)"
declare -A PEAK=()
samples=0
deadline=$(( $(date +%s) + SECONDS_TO_SAMPLE ))
while [[ $(date +%s) -lt $deadline ]]; do
  total=0
  for pid in "${ALL[@]}"; do
    kb="$(rss_kb "$pid")"
    total=$(( total + kb ))
    if [[ "${PEAK[$pid]:-0}" -lt "$kb" ]]; then PEAK["$pid"]="$kb"; fi
  done
  echo "$total" >> "$TOTALS_FILE"
  samples=$(( samples + 1 ))
  sleep "$INTERVAL"
done

declare -A END_TICKS=()
for pid in "${ALL[@]}"; do END_TICKS["$pid"]="$(cpu_ticks "$pid")"; done

# ── report ───────────────────────────────────────────────────────────────────
median_kb="$(sort -n "$TOTALS_FILE" | awk '{v[NR]=$1} END{print (NR%2)?v[(NR+1)/2]:int((v[NR/2]+v[NR/2+1])/2)}')"
peak_total=0
for pid in "${ALL[@]}"; do peak_total=$(( peak_total + ${PEAK[$pid]:-0} )); done
rm -f "$TOTALS_FILE"

cpu_total_ticks=0
{
  echo "per-process (peak RSS, CPU-seconds over the window)"
  for pid in "${ALL[@]}"; do
    delta=$(( ${END_TICKS[$pid]:-0} - ${START_TICKS[$pid]:-0} ))
    cpu_total_ticks=$(( cpu_total_ticks + delta ))
    printf '  %-28s %8.1f MiB  %6.2f s\n' \
      "${PID_NAME[$pid]:-?} ($pid)" \
      "$(awk -v k="${PEAK[$pid]:-0}" 'BEGIN{print k/1024}')" \
      "$(awk -v t="$delta" -v c="$CLK_TCK" 'BEGIN{print t/c}')"
  done
  echo
  printf 'processes:          %d\n' "${#ALL[@]}"
  printf 'samples:            %d\n' "$samples"
  printf 'steady-state RSS:   %.1f MiB (median of summed samples)\n' \
    "$(awk -v k="$median_kb" 'BEGIN{print k/1024}')"
  printf 'peak RSS:           %.1f MiB (sum of per-process peaks)\n' \
    "$(awk -v k="$peak_total" 'BEGIN{print k/1024}')"
  printf 'CPU-seconds:        %.2f over %ss\n' \
    "$(awk -v t="$cpu_total_ticks" -v c="$CLK_TCK" 'BEGIN{print t/c}')" "$SECONDS_TO_SAMPLE"
  echo
  echo "Note: peak RSS sums each process's own peak, which slightly overstates a true"
  echo "simultaneous peak -- the peaks need not coincide. Stated rather than smoothed,"
  echo "because the alternative is sampling the sum and missing short spikes entirely."
  echo
  echo "To make CPU comparable across routers, divide by the request count the load tool"
  echo "reported for the same window: CPU-seconds per 1000 requests is the honest"
  echo "efficiency figure. Instantaneous CPU%% is not, and neither is this number alone."
} | tee -a "$RESULT"

echo "written to $RESULT"

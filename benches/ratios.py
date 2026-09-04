#!/usr/bin/env python3
"""Compare two benchmark result files cell by cell.

The ratios quoted in README.md and BENCHMARKS.md come from here rather than from arithmetic done
by hand in prose. Both inputs are raw `benches/run.sh` output, so a reader can re-derive every
published figure from the files in `benches/results/`:

    benches/ratios.py benches/results/<baseline>.txt benches/results/<nullrouter>.txt

It refuses to summarise a partial comparison. An earlier draft quoted a median over eleven cells
because one cell had been rejected by the harness's own sanity check and remeasured by hand
afterwards; a median over a different set of cells than the one claimed is not the median claimed.
Cells present in one file and not the other are reported and excluded from the summary, and the
summary line says how many cells it covers.
"""

import re
import sys

CELL = re.compile(
    r"^(?P<name>S\d[\w-]+)\s+c=(?P<concurrency>\d+)\s+"
    r"through=(?P<through>[\d.]+)\s+direct=(?P<direct>[\d.]+)\s+overhead=(?P<overhead>-?[\d.]+)"
)


def cells(path):
    """Parse the measured cells out of one run file, keyed by (scenario, concurrency)."""
    found = {}
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = CELL.match(line)
            if match:
                key = (match["name"], match["concurrency"])
                found[key] = (
                    float(match["through"]),
                    float(match["direct"]),
                    float(match["overhead"]),
                )
    return found


def median(values):
    ordered = sorted(values)
    count = len(ordered)
    if count == 0:
        return float("nan")
    middle = count // 2
    if count % 2:
        return ordered[middle]
    return (ordered[middle - 1] + ordered[middle]) / 2


def main(argv):
    if len(argv) != 3:
        print(f"usage: {argv[0]} <baseline.txt> <candidate.txt>", file=sys.stderr)
        return 2
    baseline, candidate = cells(argv[1]), cells(argv[2])

    only_baseline = sorted(set(baseline) - set(candidate))
    only_candidate = sorted(set(candidate) - set(baseline))
    for key in only_baseline:
        print(f"note: {key[0]} c={key[1]} measured only in {argv[1]}; excluded")
    for key in only_candidate:
        print(f"note: {key[0]} c={key[1]} measured only in {argv[2]}; excluded")

    shared = sorted(set(baseline) & set(candidate), key=lambda key: (int(key[1]), key[0]))
    overheads, end_to_ends = [], []
    for key in shared:
        base_through, _, base_overhead = baseline[key]
        cand_through, _, cand_overhead = candidate[key]
        # A zero or negative overhead means the candidate measured no cost at all; a ratio would be
        # meaningless or infinite, so it is reported and left out rather than quietly inflating a max.
        if cand_overhead <= 0:
            print(f"note: {key[0]} c={key[1]} candidate overhead {cand_overhead}ms; excluded")
            continue
        overhead_ratio = base_overhead / cand_overhead
        end_to_end_ratio = base_through / cand_through
        overheads.append((overhead_ratio, key))
        end_to_ends.append((end_to_end_ratio, key))
        print(
            f"{key[0]:28s} c={key[1]:2s} "
            f"overhead {cand_overhead:8.3f} vs {base_overhead:9.3f} = {overhead_ratio:6.2f}x   "
            f"end-to-end {cand_through:8.3f} vs {base_through:9.3f} = {end_to_end_ratio:6.2f}x"
        )

    if not overheads:
        print("no comparable cells", file=sys.stderr)
        return 1

    def summarise(label, pairs):
        low, high = min(pairs), max(pairs)
        print(
            f"{label}: min {low[0]:.2f}x ({low[1][0]} c={low[1][1]}), "
            f"median {median([ratio for ratio, _ in pairs]):.2f}x, "
            f"max {high[0]:.2f}x ({high[1][0]} c={high[1][1]}) "
            f"over {len(pairs)} cells"
        )

    print()
    summarise("router overhead", overheads)
    summarise("end-to-end     ", end_to_ends)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

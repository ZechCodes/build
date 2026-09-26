#!/usr/bin/env python3
"""#166: one line per soak log: round trips and the packet counters around the run."""
import json, re, statistics, sys

def pct(values, p):  # the soak's own percentile (floor index), so the numbers agree
    v = sorted(values)
    return v[min(len(v) - 1, int(p / 100 * len(v)))]

def counters(text, when):
    m = re.search(rf"counters {when}: bridge_rx_tx=(\d+) (\d+) turn_rx_tx=(\d+) (\d+)", text)
    return tuple(map(int, m.groups())) if m else None

for path in sys.argv[1:]:
    text = open(path).read()
    rtts = json.loads(re.search(r"^rtts\s+(\[.*\])$", text, re.M).group(1))
    path_line = re.search(r"^path\s+(\S+)", text, re.M).group(1)
    loads = re.findall(r"loadavg (?:before|after): (\S+ \S+ \S+)", text)
    before, after = counters(text, "before"), counters(text, "after")
    d = [a - b for a, b in zip(after, before)] if before and after else None
    n = len(rtts)
    under10 = sum(1 for r in rtts if r < 10)
    print(f"{path.split('/')[-1]:<22} {path_line:<12} n={n} min={min(rtts):.1f} p50={pct(rtts,50):.1f} "
          f"p95={pct(rtts,95):.1f} max={max(rtts):.1f} mean={statistics.mean(rtts):.1f} ms  <10ms={under10}/{n}  "
          f"bridge rx/tx +{d[0]}/+{d[1]} ({d[0]/n:.2f}/{d[1]/n:.2f} per ping)  coturn rx/tx +{d[2]}/+{d[3]}  "
          f"load {loads[0]} → {loads[-1]}" if d else "")

# Summarize bench.sh's `.bench` files: the 60-frame windows after the first
# ten seconds, as the median of each window's mean and the worst window max.
import re, statistics, sys

def nums(line, key):
    m = re.search(key + r'="([0-9.]+)(?: max ([0-9.]+))?', line)
    return (float(m.group(1)), float(m.group(2)) if m.group(2) else None) if m else None

for path in sys.argv[1:]:
    lines = open(path).read().splitlines()
    ui = [l for l in lines if 'bench ui' in l or 'bench rune ui' in l][10:]
    prog = [l for l in lines if 'bench program' in l][2:]
    copy = [l for l in lines if 'bench copy' in l][2:]
    def col(rows, key, which=0):
        vals = [nums(l, key) for l in rows]
        return [v[which] for v in vals if v and v[which] is not None]
    med = lambda xs: statistics.median(xs) if xs else float('nan')
    top = lambda xs: max(xs) if xs else float('nan')
    print(f"{path.split('/')[-1][:-6]:18} windows {len(ui):3} "
          f"interval {med(col(ui, 'interval_ms')):6.2f} (max {top(col(ui, 'interval_ms', 1)):6.2f}) "
          f"cpu {med(col(ui, 'cpu_ms')):5.2f} "
          f"lock {med(col(ui, 'lock_wait_ms')):5.3f} (max {top(col(ui, 'lock_wait_ms', 1)):5.2f}) "
          f"present {med(col(ui, 'present_ms')):5.2f} "
          f"render {med(col(prog, 'render_ms')):5.2f} (max {top(col(prog, 'render_ms', 1)):6.2f}) "
          f"copy {med(col(copy, 'copy_ms')):5.2f}")

#!/usr/bin/env python3
"""Run the whole test suite in a few minutes.

    python3 tools/test_all.py            # build every test, run 4 programs at a time
    python3 tools/test_all.py --jobs 6   # more at once
    python3 tools/test_all.py --serial   # one at a time, as `cargo test` does

`cargo test --workspace` stops at the first test program that does not
compile, and then runs the ~170 test programs one after another. This builds
them all with `--keep-going` (a broken one is reported, the rest still run)
and runs several programs at once; each still runs its own tests on all cores.
At the end: the time, the counts, every failing test by name, and the slowest
programs.
"""
import argparse
import concurrent.futures as cf
import json
import os
import re
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def build():
    """Every test program: (path, crate, target, crate dir), and the targets that failed to build."""
    p = subprocess.run(
        ["cargo", "build", "--workspace", "--tests", "--keep-going", "--message-format=json"],
        cwd=ROOT, capture_output=True, text=True,
    )
    programs = []
    for line in p.stdout.splitlines():
        try:
            m = json.loads(line)
        except ValueError:
            continue
        if m.get("reason") != "compiler-artifact" or not m.get("executable") or not m["profile"].get("test"):
            continue
        t = m["target"]
        crate = m["package_id"].split("#")[0].split("/")[-1]
        programs.append((m["executable"], crate, f'{t["kind"][0]}:{t["name"]}', os.path.dirname(m["manifest_path"])))
    broken = re.findall(r"error: could not compile `([^`]+)` \(([^)]+)\)", p.stderr)
    return programs, broken


def run(program):
    exe, crate, target, cwd = program
    start = time.time()
    try:
        p = subprocess.run([exe, "-q"], cwd=cwd, capture_output=True, text=True, timeout=1800)
        out, code = p.stdout + p.stderr, p.returncode
    except subprocess.TimeoutExpired:
        out, code = "", "timed out after 30 min"
    counts = [0, 0, 0]
    for m in re.finditer(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", out):
        for i in range(3):
            counts[i] += int(m[i + 1])
    failed = re.findall(r"^    (\S+)$", out.split("failures:")[-1], re.M) if counts[1] else []
    return crate, target, time.time() - start, counts, failed, code


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--jobs", type=int, default=4, help="test programs at once (default 4)")
    ap.add_argument("--serial", action="store_true", help="one program at a time")
    args = ap.parse_args()
    jobs = 1 if args.serial else max(1, args.jobs)

    began = time.time()
    print("Building every test…", flush=True)
    programs, broken = build()
    built = time.time() - began
    print(f"  {len(programs)} test programs in {built:.0f}s", flush=True)
    for crate, what in broken:
        print(f"  does not compile: {crate} ({what})")

    print(f"Running, {jobs} at a time…", flush=True)
    results = []
    with cf.ThreadPoolExecutor(jobs) as pool:
        for r in pool.map(run, programs):
            results.append(r)
            if r[3][1] or r[5] not in (0, 101):
                print(f"  {r[0]} {r[1]}: {r[3][1]} failed" + ("" if r[5] in (0, 101) else f" ({r[5]})"), flush=True)
    ran = time.time() - began - built

    passed, failed, ignored = (sum(r[3][i] for r in results) for i in range(3))
    print(f"\n{passed} passed, {failed} failed, {ignored} ignored · build {built:.0f}s + run {ran:.0f}s = {(built + ran) / 60:.1f} min")
    if failed:
        print("\nFailing tests:")
        for crate, target, _, _, names, _ in results:
            for name in names:
                print(f"  {crate} {target}  {name}")
    print("\nSlowest programs:")
    for crate, target, secs, *_ in sorted(results, key=lambda r: -r[2])[:5]:
        print(f"  {secs:6.1f}s  {crate} {target}")
    sys.exit(1 if failed or broken else 0)


if __name__ == "__main__":
    main()

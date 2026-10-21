#!/usr/bin/env python3
"""Compare the outcome of every test between two sets of `al-runner test --junit` files.

    tools/bucket_diff.py --a solo-1.xml solo-2.xml ... --b combined.xml

Each test is keyed by (codeunit id, name). The script prints how many tests moved between
pass, fail and not run, and groups the differing tests by codeunit and by the reason of side B.
Used for docs/alrunner-buckets.md ("Alone and together").
"""
import argparse
import collections
import xml.etree.ElementTree as ET


def load(paths):
    out = {}
    for path in paths:
        try:
            root = ET.parse(path).getroot()
        except ET.ParseError:  # an empty file: the selection matched no test
            continue
        for suite in root.iter("testsuite"):
            ids = [p.get("value") for p in suite.iter("property") if p.get("name") == "codeunit"]
            cu = ids[0] if ids else suite.get("name")
            for case in suite.iter("testcase"):
                skipped, failed = case.find("skipped"), case.find("failure")
                if failed is None:
                    failed = case.find("error")
                if skipped is not None:
                    outcome = ("not run", skipped.get("message") or "")
                elif failed is not None:
                    outcome = ("failed", failed.get("message") or "")
                else:
                    outcome = ("passed", "")
                out[(cu, case.get("name"))] = outcome
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--a", nargs="+", required=True, help="JUnit files of side A (for example one per bucket)")
    ap.add_argument("--b", nargs="+", required=True, help="JUnit files of side B (for example all buckets in one run)")
    args = ap.parse_args()
    a, b = load(args.a), load(args.b)
    print(f"A: {len(a)} tests, B: {len(b)} tests, only in A: {len(set(a) - set(b))}, only in B: {len(set(b) - set(a))}")
    for name, side in (("A", a), ("B", b)):
        c = collections.Counter(v[0] for v in side.values())
        print(f"{name}: {c['passed']} passed, {c['failed']} failed, {c['not run']} not run")
    moved = collections.Counter()
    groups = collections.defaultdict(list)
    for k in a.keys() & b.keys():
        moved[(a[k][0], b[k][0])] += 1
        if a[k][0] != b[k][0]:
            groups[(k[0], a[k][0], b[k][0], b[k][1][:140])].append(k[1])
    for (x, y), n in sorted(moved.items()):
        print(f"{n:6}  {x} -> {y}")
    for (cu, x, y, why), names in sorted(groups.items()):
        print(f"\ncodeunit {cu}: {len(names)} tests {x} -> {y}\n  B says: {why}\n  e.g. {sorted(names)[0]}")


if __name__ == "__main__":
    main()

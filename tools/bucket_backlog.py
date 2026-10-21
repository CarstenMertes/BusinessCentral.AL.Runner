#!/usr/bin/env python3
"""Turns the junit files of one bucket run into a backlog of work items.

    tools/bucket_backlog.py OUT_DIR [--previous PREV_DIR] [--zip-dir DIR]

OUT_DIR holds one `<bucket>.xml` per bucket (written by tools/bucket_backlog.sh, plus an optional
buckets.tsv with exit code and seconds). Writes OUT_DIR/backlog.md and OUT_DIR/backlog.json.

An item is a group of not-run tests with the same folded refusal reason, or of failed tests with
the same folded failure message. Folding replicates `normalise_reason` of
crates/al-testrun/src/refusals.rs (--top-refusals): the routine prefix goes, quoted names become
<name>, numbers become N. Each item lists, per bucket, the codeunit ids, up to 10 test names and
the command that reruns just those codeunits.
"""
import argparse
import collections
import json
import os
import re
import shlex
import xml.etree.ElementTree as ET

DEFAULT_ZIP_DIR = "/home/stefan/.bcartifacts.cache/sandbox/27.5.46862.48827/platform/Applications/BaseApp/Test"
NAMES_PER_BUCKET = 10


# --- folding (same rules as refusals.rs) -----------------------------------------------------

def _is_site_prefix(head):
    if head == "OnRun":
        return True
    if head.startswith("handler ") and not any(c in head for c in ".`("):
        return True
    if len(head) > 2 and head.startswith("`") and head.endswith("`") and "`" not in head[1:-1]:
        return True
    if head.startswith("DotNet") or head.startswith("builtin ") or any(c in head for c in "`('"):
        return False
    if "." in head:
        obj, routine = head.rsplit(".", 1)
        return bool(obj) and bool(routine) and all(c.isalnum() or c == "_" for c in routine) and not routine[0].isdigit()
    return False


def strip_site(raw):
    rest = raw.strip()
    for _ in range(4):
        if ": " not in rest:
            break
        head, tail = rest.split(": ", 1)
        if not tail.strip() or not _is_site_prefix(head):
            break
        rest = tail.lstrip()
    return rest


def fold_quotes(s):
    out, i, n = [], 0, len(s)
    while i < n:
        c = s[i]
        if c in "'`\"" and (i == 0 or not s[i - 1].isalnum()):
            close = next((j for j in range(i + 1, n) if s[j] == c and (j + 1 >= n or not s[j + 1].isalnum())), None)
            if close is not None:
                out.append("<name>")
                i = close + 1
                continue
        out.append(c)
        i += 1
    return "".join(out)


def fold_numbers(s):
    word = lambda c: c.isalpha() or c == "_"
    out, i, n = [], 0, len(s)
    while i < n:
        if not ("0" <= s[i] <= "9"):
            out.append(s[i])
            i += 1
            continue
        j = i
        while j < n and "0" <= s[j] <= "9":
            j += 1
        in_word = (i > 0 and word(s[i - 1])) or (j < n and word(s[j]))
        out.append(s[i:j] if in_word else "N")
        i = j
    return "".join(out)


def normalise_reason(raw):
    first = raw.strip().split("\n", 1)[0]
    return fold_numbers(fold_quotes(strip_site(first)))


# --- reading ---------------------------------------------------------------------------------

def load_bucket(path):
    """Returns {(codeunit, test): (outcome, message, seconds)} for one junit file."""
    out = {}
    try:
        root = ET.parse(path).getroot()
    except ET.ParseError:
        return out
    for suite in root.iter("testsuite"):
        ids = [p.get("value") for p in suite.iter("property") if p.get("name") == "codeunit"]
        cu = ids[0] if ids else suite.get("name")
        for case in suite.iter("testcase"):
            skipped, failed = case.find("skipped"), case.find("failure")
            if failed is None:
                failed = case.find("error")
            t = float(case.get("time") or 0)
            if skipped is not None:
                out[(cu, case.get("name"))] = ("not run", skipped.get("message") or "", t)
            elif failed is not None:
                out[(cu, case.get("name"))] = ("failed", failed.get("message") or "", t)
            else:
                out[(cu, case.get("name"))] = ("passed", "", t)
    return out


def load_dir(d):
    return {f[:-4]: load_bucket(os.path.join(d, f)) for f in sorted(os.listdir(d)) if f.endswith(".xml")}


def load_tsv(d):
    res = {}
    p = os.path.join(d, "buckets.tsv")
    if os.path.exists(p):
        for line in open(p).read().splitlines()[1:]:
            b, code, secs = line.split("\t")
            res[b] = {"exit": int(code), "seconds": int(secs)}
    return res


def cu_key(cu):
    return (0, int(cu)) if str(cu).isdigit() else (1, str(cu))


# --- building --------------------------------------------------------------------------------

def build(buckets, previous, tsv, zip_dir):
    per_bucket = {}
    items = {}
    slow = []
    for b, tests in buckets.items():
        c = collections.Counter(v[0] for v in tests.values())
        per_bucket[b] = {"passed": c["passed"], "failed": c["failed"], "not_run": c["not run"],
                         "total": len(tests), **tsv.get(b, {})}
        for (cu, name), (outcome, msg, t) in tests.items():
            slow.append((t, b, cu, name, outcome))
            if outcome == "passed":
                continue
            key = (outcome, normalise_reason(msg) or "(no message)")
            it = items.setdefault(key, {"kind": outcome, "reason": key[1], "example": msg.split("\n", 1)[0][:300],
                                        "tests": 0, "buckets": {}})
            it["tests"] += 1
            bk = it["buckets"].setdefault(b, {"tests": 0, "codeunits": collections.Counter(), "names": []})
            bk["tests"] += 1
            bk["codeunits"][cu] += 1
            if len(bk["names"]) < NAMES_PER_BUCKET:
                bk["names"].append(f"{cu} {name}")
    out_items = []
    for it in sorted(items.values(), key=lambda i: (-i["tests"], i["kind"], i["reason"])):
        buckets_out = []
        for b, bk in sorted(it["buckets"].items(), key=lambda kv: (-kv[1]["tests"], kv[0])):
            cus = sorted(bk["codeunits"], key=cu_key)
            zp = os.path.join(zip_dir, f"Tests-{b}.Source.zip")
            cmd = "al-runner test " + shlex.quote(zp) + "".join(f" --codeunit {c}" for c in cus)
            buckets_out.append({"bucket": b, "tests": bk["tests"], "codeunits": [int(c) if str(c).isdigit() else c for c in cus],
                                "names": bk["names"], "rerun": cmd})
        out_items.append({**{k: it[k] for k in ("kind", "reason", "example", "tests")}, "buckets": buckets_out})
    slow.sort(key=lambda s: -s[0])
    slowest = [{"seconds": t, "bucket": b, "codeunit": cu, "test": n, "outcome": o} for t, b, cu, n, o in slow[:20]]
    result = {"buckets": per_bucket, "items": out_items, "slowest": slowest}
    if previous is not None:
        lost, gained = [], collections.Counter()
        for b, tests in buckets.items():
            prev = previous.get(b, {})
            for k, (o, m, _) in tests.items():
                po = prev.get(k, ("missing",))[0]
                if o == "passed" and po not in ("passed", "missing"):
                    gained[b] += 1
            for k, (po, _, _) in prev.items():
                if po == "passed" and tests.get(k, ("missing",))[0] != "passed":
                    now = tests.get(k, ("missing", ""))
                    lost.append({"bucket": b, "codeunit": k[0], "test": k[1], "now": now[0], "message": now[1].split("\n", 1)[0][:300]})
        for b in previous:  # a bucket that vanished loses all its passes
            if b not in buckets:
                for k, (po, _, _) in previous[b].items():
                    if po == "passed":
                        lost.append({"bucket": b, "codeunit": k[0], "test": k[1], "now": "missing", "message": ""})
        lost.sort(key=lambda x: (x["bucket"], cu_key(x["codeunit"]), x["test"]))
        result["lost_passes"] = lost
        result["new_passes"] = dict(sorted(gained.items()))
    return result


# --- rendering -------------------------------------------------------------------------------

def render(r, title):
    L = [f"# Bucket backlog: {title}", ""]
    tot = collections.Counter()
    for v in r["buckets"].values():
        for k in ("passed", "failed", "not_run", "total", "seconds"):
            tot[k] += v.get(k, 0)
    L += [f"Totals: {tot['total']} tests, {tot['passed']} passed, {tot['failed']} failed, {tot['not_run']} not run; "
          f"sum of bucket wall time {tot['seconds']} s.", ""]
    L += ["## Buckets", "", "| bucket | total | passed | failed | not run | exit | seconds |", "|---|---:|---:|---:|---:|---:|---:|"]
    for b, v in r["buckets"].items():
        L.append(f"| {b} | {v['total']} | {v['passed']} | {v['failed']} | {v['not_run']} | {v.get('exit', '')} | {v.get('seconds', '')} |")
    L.append("")
    if "lost_passes" in r:
        L += [f"## Lost passes ({len(r['lost_passes'])})", ""]
        L += ["Tests that passed in the previous run and do not pass now.", ""]
        for x in r["lost_passes"]:
            L.append(f"- {x['bucket']} / {x['codeunit']} / {x['test']}: now {x['now']}" + (f" ({x['message']})" if x["message"] else ""))
        L += ["", "## New passes per bucket", ""]
        L += [f"- {b}: {n}" for b, n in r["new_passes"].items()] or ["- none"]
        L.append("")
    L += [f"## Backlog ({len(r['items'])} items, ranked by test count)", ""]
    for n, it in enumerate(r["items"], 1):
        L += [f"### {n}. [{it['kind']}] {it['reason']}", "", f"{it['tests']} tests in {len(it['buckets'])} buckets. Example as raised: `{it['example']}`", ""]
        for b in it["buckets"]:
            cus = ", ".join(str(c) for c in b["codeunits"])
            L += [f"- **{b['bucket']}**: {b['tests']} tests, codeunits {cus}"]
            L += [f"  - {nm}" for nm in b["names"]]
            L += [f"  - rerun: `{b['rerun']}`"]
        L.append("")
    L += ["## 20 slowest tests", "", "| seconds | bucket | codeunit | test | outcome |", "|---:|---|---|---|---|"]
    L += [f"| {s['seconds']:.3f} | {s['bucket']} | {s['codeunit']} | {s['test']} | {s['outcome']} |" for s in r["slowest"]]
    return "\n".join(L) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out_dir")
    ap.add_argument("--previous", help="output dir of an earlier run, for lost and new passes")
    ap.add_argument("--zip-dir", default=DEFAULT_ZIP_DIR, help="folder of the Tests-*.Source.zip files, for the rerun commands")
    a = ap.parse_args()
    buckets = load_dir(a.out_dir)
    previous = load_dir(a.previous) if a.previous else None
    r = build(buckets, previous, load_tsv(a.out_dir), a.zip_dir)
    with open(os.path.join(a.out_dir, "backlog.json"), "w") as f:
        json.dump(r, f, indent=1)
    with open(os.path.join(a.out_dir, "backlog.md"), "w") as f:
        f.write(render(r, os.path.basename(os.path.abspath(a.out_dir))))
    print(f"{len(r['items'])} backlog items; wrote backlog.md and backlog.json in {a.out_dir}")


if __name__ == "__main__":
    main()

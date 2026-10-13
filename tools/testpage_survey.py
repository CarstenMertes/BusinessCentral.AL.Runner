#!/usr/bin/env python3
"""Survey of the corpus tests that drive a page (docs/adr/012-testpage-runtime.md).

Usage: python3 tools/testpage_survey.py corpus/tests/al-language [--files]

Heuristic and static: it reads the AL text, not a parse tree, and counts a
[Test] procedure as page-driven when its body or one of its page handlers
declares a TestPage / TestRequestPage variable, or when it names a
[PageHandler] / [ModalPageHandler]. The tiers are ceilings (what a runtime of
that tier could at most reach on the page axis), not predictions.
"""
import collections
import glob
import os
import re
import sys

root = sys.argv[1]
show_files = "--files" in sys.argv

srcs = {}
for f in glob.glob(os.path.join(root, "**", "*.al"), recursive=True):
    srcs[f] = open(f, encoding="utf-8-sig").read()

OBJ = re.compile(
    r"^\s*(page|table|codeunit|pageextension|tableextension|report|query|enum|xmlport|enumextension|interface|permissionset|profile)\s+\d*\s*(\"[^\"]+\"|\w+)",
    re.M,
)
pages = {}
for f, src in srcs.items():
    for m in re.finditer(r"^\s*page\s+\d+\s+(\"[^\"]+\"|\w+)", src, re.M):
        nxt = OBJ.search(src, m.end())
        pages[m.group(1).strip('"')] = src[m.start() : nxt.start() if nxt else len(src)]


def page_flags(name):
    t = pages.get(name)
    if t is None:
        return {"outside"}
    fl = set()
    if re.search(r"^\s*(part|systempart)\s*\(", t, re.M):
        fl.add("part")
    if re.search(r"AutoFormat|DecimalPlaces|BlankZero|BlankNumbers", t):
        fl.add("format-props")
    if re.search(r"SourceTableTemporary\s*=\s*true", t):
        fl.add("temporary")
    return fl


proc_re = re.compile(r"((?:\s*\[[^\]]*\]\s*)+)\s*(?:local\s+)?procedure\s+(\"[^\"]+\"|\w+)\s*\(")
rows = []
for f, src in srcs.items():
    if not re.search(r"^\s*codeunit\s+\d+", src, re.M):
        continue
    handlers = {}
    for hm in re.finditer(r"\[(\w+Handler)(?:\([^)]*\))?\]\s*(?:local\s+)?procedure\s+(\"[^\"]+\"|\w+)\s*\(", src):
        handlers[hm.group(2).strip('"')] = hm.group(1)
    for pm in proc_re.finditer(src):
        if "[Test]" not in pm.group(1):
            continue
        rest = src[pm.end() :]
        nx = proc_re.search(rest)
        body = rest[: nx.start()] if nx else rest
        hk, hbody = set(), ""
        h = re.search(r"HandlerFunctions\('([^']*)'", pm.group(1))
        for hn in (h.group(1).split(",") if h else []):
            hn = hn.strip()
            hk.add(handlers.get(hn, "?"))
            m2 = re.search(r"procedure\s+" + re.escape(hn) + r"\s*\(.*?(?=\n\s*(?:\[[^\]]*\]\s*)*(?:local\s+)?procedure\s|\Z)", src, re.S)
            hbody += m2.group(0) if m2 else ""
        tps = {t.strip('"') for t in re.findall(r":\s*Test(?:Page|RequestPage)\s+(\"[^\"]+\"|\w+)", body + hbody)}
        if not tps and not (hk & {"PageHandler", "ModalPageHandler"}):
            continue
        pf = set()
        for t in tps:
            pf |= page_flags(t)
        all_text = body + hbody
        fl = set()
        if "outside" in pf:
            fl.add("outside")  # Base App page or request page of a report
        if "part" in pf or "TestPart" in all_text:
            fl.add("part")
        if hk:
            fl.add("handler")
        if re.search(r"Page\.Run|\.RunModal\(|Page\.Update|RunObject", all_text):
            fl.add("page-run")
        if re.search(r"\b(Message|Confirm|StrMenu)\(", all_text):
            fl.add("dialog")
        if "RunPageBackgroundTask" in all_text:
            fl.add("bgtask")
        if "format-props" in pf:
            fl.add("format")
        if re.search(r"EventSubscriber|BindSubscription", src):
            fl.add("events")
        if "ALTFixtureCleanup" in src:
            fl.add("cleanup")
        rows.append((os.path.relpath(f, root), pm.group(2).strip('"'), fl))

tier_of = lambda fl: (
    "outside the workspace (Base App page / request page)"
    if "outside" in fl
    else "parts (TestPart / part())"
    if "part" in fl
    else "modal + page handlers, Page.Run*, dialogs"
    if fl & {"handler", "page-run", "dialog"}
    else "background tasks"
    if "bgtask" in fl
    else "core: TestPage on in-workspace pages, no handler/part"
)
c = collections.Counter(tier_of(fl) for _, _, fl in rows)
print(f"page-driven [Test] procedures: {len(rows)}")
for k, v in c.most_common():
    print(f"  {v:5d}  {k}")
core = [r for r in rows if tier_of(r[2]).startswith("core")]
print(f"core tier: {len(core)}; of which also need table events / BindSubscription (file has EventSubscriber/BindSubscription): {sum('events' in r[2] for r in core)};")
print(f"           call ALTFixtureCleanup (DeleteAll on tables with subscribers): {sum('cleanup' in r[2] for r in core)};")
print(f"           on pages with AutoFormat/DecimalPlaces/Blank* properties: {sum('format' in r[2] for r in core)}")
if show_files:
    by = collections.Counter(r[0] for r in core)
    for f, n in by.most_common():
        print(f"{n:4d} {f}")

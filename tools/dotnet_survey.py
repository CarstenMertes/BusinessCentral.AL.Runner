#!/usr/bin/env python3
"""Which DotNet types do dependency-app procedures reach from corpus tests? (ADR-013)

usage: tools/dotnet_survey.py <corpus-app-dir> <dep-source-dir>... [--top N] [--no-events] [--tests]

<corpus-app-dir> holds the corpus `.al` files (for example
corpus/tests/al-language). Each <dep-source-dir> holds extracted AL source of
a dependency app (System Application, Base Application, Business Foundation,
...). The source is read from a local cache and is never copied into the
repository. Nothing is written except the report on stdout.

Method (textual, so approximate; see docs/dotnet-examples.md):
 1. Parse every `.al` file into objects and procedures/triggers.
 2. Map every `DotNet` alias to its .NET type from the `dotnet { }` blocks.
 3. A procedure "uses" a DotNet variable when the variable is declared in the
    procedure header or `var` block, or at object level, and its name appears
    in the procedure body.
 4. Call edges: `Var.Method(` where `Var` is declared as `Codeunit/Record/Page/
    Report/XmlPort/Query "X"` (procedures of X; `Var.Run` reaches OnRun);
    bare `Method(` inside the same object; `Codeunit.Run(Codeunit::"X")`;
    `Rec.Insert(true)` and its siblings reach the table trigger; event
    publishers reach their subscribers, and so do database events after
    `Insert(true)` and its siblings (`--no-events` turns the subscriber edges
    off).
 5. Roots are the `[Test]` procedures of the corpus app. A test "reaches" a
    DotNet use when a path of call edges leads to a procedure that uses one.
Under-counts: calls through interfaces, `RecordRef`, `Variant`, `Validate`,
page and report triggers, and `TableRelation`. Over-counts: calls matched by
procedure name only, and every subscriber of an event is assumed to fire.
"""
import collections
import os
import re
import sys

args = [a for a in sys.argv[1:] if not a.startswith('--')]
top = int(sys.argv[sys.argv.index('--top') + 1]) if '--top' in sys.argv else 25
if '--top' in sys.argv:
    args.remove(sys.argv[sys.argv.index('--top') + 1])
use_events = '--no-events' not in sys.argv
list_tests = '--tests' in sys.argv
corpus_dir, dep_dirs = args[0], args[1:]

KIND = r'(?:codeunit|table|page|report|query|xmlport|enum|interface|tableextension|pageextension|enumextension|reportextension|permissionset|permissionsetextension|controladdin|profile|entitlement)'
OBJ_RE = re.compile(r'^\s*(' + KIND + r')\s+(\d+)?\s*("[^"]+"|[\w.]+)', re.I)
PROC_RE = re.compile(r'^(\s*)(?:\[[^\]]*\]\s*)*(?:(?:local|internal|protected)\s+)?(procedure|trigger)\s+("[^"]+"|\w+)', re.I)
VAR_RE = re.compile(r'^\s*(?:var\s+)?("[^"]+"|\w+)\s*:\s*(?:temporary\s+)?(Codeunit|Record|Page|Report|XmlPort|Query|DotNet|Interface|Enum)\s+("(?:[^"]+)"|[\w.]+)', re.I)
HDR_VAR_RE = re.compile(r'(?:var\s+)?("[^"]+"|\w+)\s*:\s*(?:temporary\s+)?(Codeunit|Record|Page|Report|XmlPort|Query|DotNet|Interface|Enum)\s+("(?:[^"]+)"|[\w.]+)', re.I)
CALL_RE = re.compile(r'("[^"]+"|\w+)\s*\.\s*("[^"]+"|\w+)\s*(?:\(|;|\b)')
BARE_RE = re.compile(r'(?<![\w.":])("[^"]+"|\w+)\s*\(')
OBJREF_RE = re.compile(r'\b(Codeunit|Page|Report|XmlPort|Query|Database)\s*::\s*("[^"]+"|[\w.]+)', re.I)
SUB_RE = re.compile(r'\[\s*(?:EventSubscriber|ExternalBusinessEvent)\s*\(\s*ObjectType::(\w+)\s*,\s*(?:\w+)::("[^"]+"|[\w.]+)\s*,\s*\'?"?([\w ]+?)"?\'?\s*,', re.I)
DN_TYPE_RE = re.compile(r'type\(\s*"([^"]+)"\s*;\s*"?([\w.]+)"?\s*\)')
DN_ASM_RE = re.compile(r'assembly\(\s*"([^"]+)"\s*\)')


def unq(s):
    s = s.strip()
    if '"' in s:
        m = re.findall(r'"([^"]+)"', s)
        return m[-1]
    return s.split('.')[-1]


class Proc:
    __slots__ = ('obj', 'name', 'kind', 'text', 'is_test', 'vars', 'dn', 'line')

    def __init__(self, obj, name, kind, text):
        self.obj, self.name, self.kind, self.text = obj, name, kind, text
        self.is_test = False
        self.vars = {}
        self.dn = set()


class Obj:
    def __init__(self, kind, name, app, path):
        self.kind, self.name, self.app, self.path = kind.lower(), name, app, path
        self.procs = []
        self.gvars = {}
        self.subs = []  # (event key, proc)


aliases = {}  # (app, alias) -> (assembly, type)
objects = []
by_name = collections.defaultdict(list)  # (kind, name.lower()) -> [Obj]


def parse_file(path, app, corpus):
    try:
        text = open(path, encoding='utf-8-sig', errors='replace').read()
    except OSError:
        return
    if re.search(r'^dotnet\b', text, re.M):
        asm = None
        for line in text.splitlines():
            m = DN_ASM_RE.search(line)
            if m:
                asm = m[1]
            m = DN_TYPE_RE.search(line)
            if m:
                aliases[(app, m[2].lower())] = (asm, m[1])
        return
    lines = text.splitlines()
    cur = None
    i = 0
    n = len(lines)
    ranges = []
    while i < n:
        line = lines[i]
        if cur is None:
            m = OBJ_RE.match(line)
            if m:
                cur = Obj(m[1], unq(m[3]), app, path)
                cur.ext = m[1].lower().endswith('extension')
                objects.append(cur)
                by_name[(cur.kind, cur.name.lower())].append(cur)
            i += 1
            continue
        m = PROC_RE.match(line)
        if m:
            indent, pk, pname = m[1], m[2].lower(), unq(m[3])
            attrs = []
            j = i - 1
            while j >= 0 and lines[j].strip().startswith('['):
                attrs.append(lines[j])
                j -= 1
            end = i
            while end < n and not re.match('^' + indent + r'end;?\s*$', lines[end]):
                end += 1
                if end - i > 4000:
                    break
            body = '\n'.join(lines[i:end + 1])
            p = Proc(cur, pname, pk, body)
            p.line = i + 1
            all_attrs = '\n'.join(attrs) + '\n' + line
            p.is_test = bool(re.search(r'\[\s*Test\s*\]', all_attrs, re.I))
            # header variables up to first "begin"/"var" keyword is overkill: scan all declarations
            for vm in HDR_VAR_RE.finditer(body.split('begin')[0]):
                p.vars[unq(vm[1]).lower()] = (vm[2].lower(), unq(vm[3]))
            for sm in SUB_RE.finditer('\n'.join(attrs)):
                cur.subs.append(((sm[1].lower(), unq(sm[2]).lower(), sm[3].strip().lower()), p))
            cur.procs.append(p)
            ranges.append((i, end))
            i = end + 1
            continue
        i += 1
    # object-level variables = declarations outside procedure ranges
    inside = set()
    for a, b in ranges:
        inside.update(range(a, b + 1))
    for k, line in enumerate(lines):
        if k in inside or cur is None:
            continue
        m = VAR_RE.match(line)
        if m:
            cur.gvars[unq(m[1]).lower()] = (m[2].lower(), unq(m[3]))


def walk(d, app, corpus=False):
    for dp, _, fs in os.walk(d):
        for f in fs:
            if f.lower().endswith('.al'):
                parse_file(os.path.join(dp, f), app, corpus)


walk(corpus_dir, 'corpus', True)
corpus_objs = len(objects)
for d in dep_dirs:
    walk(d, os.path.basename(d.rstrip('/')))

# DotNet uses per procedure
dn_count = 0
for o in objects:
    for p in o.procs:
        names = dict(o.gvars)
        names.update(p.vars)
        for v, (k, t) in names.items():
            if k == 'dotnet' and re.search(r'(?<![\w"])"?' + re.escape(v) + r'"?\b', p.text, re.I):
                al = aliases.get((o.app, t.lower())) or aliases.get(('', t.lower()))
                if al is None:
                    for (a, al_name), val in aliases.items():
                        if al_name == t.lower():
                            al = val
                            break
                p.dn.add((al[1] if al else t, t))
        if p.dn:
            dn_count += 1

# call graph
subs_by_event = collections.defaultdict(list)
for o in objects:
    for key, p in o.subs:
        subs_by_event[key].append(p)

EDGE = {}


RUN_RE = re.compile(r'\bCodeunit\s*\.\s*Run\s*\(\s*Codeunit\s*::\s*("[^"]+"|[\w.]+)', re.I)
DB_RE = re.compile(r'("[^"]+"|\w+)\s*\.\s*(Insert|Modify|Delete|Rename)\s*\(\s*true\b', re.I)


def targets(p):
    """Call edges out of one procedure (see the module docstring for the rules)."""
    if p in EDGE:
        return EDGE[p]
    o = p.obj
    out = set()
    vars_ = dict(o.gvars)
    vars_.update(p.vars)
    body = p.text
    for m in CALL_RE.finditer(body):
        v = unq(m[1]).lower()
        meth = unq(m[2]).lower()
        if v in vars_:
            k, t = vars_[v]
            if k in ('codeunit', 'page', 'report', 'xmlport', 'query', 'record'):
                for t_obj in by_name.get((k if k != 'record' else 'table', t.lower()), []):
                    for q in t_obj.procs:
                        if q.kind == 'procedure' and q.name.lower() == meth:
                            out.add(q)
                        elif k == 'codeunit' and meth == 'run' and q.name.lower() == 'onrun':
                            out.add(q)
                if use_events and k in ('codeunit', 'record'):
                    out.update(subs_by_event.get(('codeunit' if k == 'codeunit' else 'table', t.lower(), meth), []))
        elif v in ('rec', 'xrec', 'currpage', 'currreport'):
            for q in o.procs:
                if q.kind == 'procedure' and q.name.lower() == meth:
                    out.add(q)
    for m in BARE_RE.finditer(body):
        meth = unq(m[1]).lower()
        for q in o.procs:
            if q.kind == 'procedure' and q.name.lower() == meth and q is not p:
                out.add(q)
        if use_events:
            out.update(subs_by_event.get((o.kind, o.name.lower(), meth), []))
    for m in RUN_RE.finditer(body):
        for t_obj in by_name.get(('codeunit', unq(m[1]).lower()), []):
            out.update(q for q in t_obj.procs if q.name.lower() == 'onrun')
    for m in DB_RE.finditer(body):
        v = unq(m[1]).lower()
        if v in vars_ and vars_[v][0] == 'record':
            t = vars_[v][1].lower()
            op = m[2].lower()
            for t_obj in by_name.get(('table', t), []):
                out.update(q for q in t_obj.procs if q.name.lower() == 'on' + op)
            if use_events:
                for pre in ('onbefore', 'onafter'):
                    out.update(subs_by_event.get(('table', t, pre + op + 'event'), []))
    out.discard(p)
    EDGE[p] = out
    return out


# reachability from each test, memoised over a DAG condensation is overkill; BFS per test with cap
tests = [p for o in objects[:corpus_objs] for p in o.procs if p.is_test]
reach_dn = collections.defaultdict(set)  # (proc) -> tests
per_test = {}
memo = {}


def reach(root):
    seen = {root}
    stack = [root]
    found = []
    while stack:
        p = stack.pop()
        if p.dn:
            found.append(p)
        for q in targets(p):
            if q not in seen:
                seen.add(q)
                stack.append(q)
    return found


proc_tests = collections.defaultdict(set)
type_tests = collections.defaultdict(set)
obj_tests = collections.defaultdict(set)
tests_with = 0
for t in tests:
    found = reach(t)
    if found:
        tests_with += 1
    for p in found:
        key = (p.obj.kind, p.obj.name, p.name)
        tid = (t.obj.name, t.name)
        proc_tests[key].add(tid)
        obj_tests[(p.obj.kind, p.obj.name)].add(tid)
        for al, _ in p.dn:
            type_tests[al].add(tid)

print(f'objects parsed: {len(objects)} ({corpus_objs} corpus); DotNet aliases: {len(aliases)}; procedures using DotNet: {dn_count}')
print(f'corpus [Test] procedures: {len(tests)}; reaching at least one DotNet use: {tests_with}')
print('\nBy .NET type (tests that reach it):')
for k, v in sorted(type_tests.items(), key=lambda kv: -len(kv[1]))[:top]:
    print(f'{len(v):6d}  {k}')
print('\nBy dependency object:')
for k, v in sorted(obj_tests.items(), key=lambda kv: -len(kv[1]))[:top]:
    print(f'{len(v):6d}  {k[0]} "{k[1]}"')
print('\nBy procedure:')
for k, v in sorted(proc_tests.items(), key=lambda kv: -len(kv[1]))[:top]:
    types = sorted({al for o in by_name.get((k[0], k[1].lower()), []) for p in o.procs if p.name == k[2] for al, _ in p.dn})
    print(f'{len(v):6d}  {k[0]} "{k[1]}" {k[2]}  [{", ".join(types)}]')

if list_tests:
    print('\nTests per dependency procedure (corpus file:line, test name):')
    for k, v in sorted(proc_tests.items(), key=lambda kv: -len(kv[1])):
        print(f'{k[0]} "{k[1]}" {k[2]}')
        for t in tests:
            if (t.obj.name, t.name) in v:
                print(f'    {os.path.relpath(t.obj.path, corpus_dir)}:{t.line}  {t.obj.name}.{t.name}')

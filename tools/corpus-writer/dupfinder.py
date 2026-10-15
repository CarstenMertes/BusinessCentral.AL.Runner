#!/usr/bin/env python3
"""Find corpus tests that assert the same fact as an existing test.

usage: dupfinder.py --corpus <corpus clone> [--new PATH ... | --new-git REF] [--min-keys N] [--min-share P] [--show]

A "fact" is the expression a test looks at, with variable names blanked and builtin names lowercased:
  Obs.Observe('a1', Format(LowerCase('ABC')))   -> lowercase('ABC')
  Assert.AreEqual('abc', LowerCase('ABC'), ...) -> lowercase('ABC')
  Observer.Observe(...) / Observer.Expect(...) pairs are read from the Observe side.
Two tests that look at the same expression (same member, same literal inputs) cover the same fact, whatever
the expected value or the variable names.

--new PATH   a file or folder (relative to the corpus clone, or absolute) holding the tests under review.
             Without --new every test is compared with every other test (a full report).
--new-git REF files ADDED since REF (git diff --diff-filter=A REF...HEAD in the corpus clone): the usual way to review a branch
--min-keys   a test with fewer facts than this is not reported (default 3)
--min-share  report a new test when at least this share of its facts already exists in OTHER files (default 0.6)
--show       also list the duplicate facts of each reported test with the first place they exist

Output: one block per reported test, then a summary. Exit code 0 always (it informs, the owner decides).
Only .al files under tests/al-language and tests/al-language-onprem are read.
"""
import argparse, os, re, sys, collections

def strip_comments(s):
    out, i, n = [], 0, len(s)
    while i < n:
        c = s[i]
        if c == "'":
            j = i + 1
            while j < n:
                if s[j] == "'":
                    if j + 1 < n and s[j + 1] == "'":
                        j += 2; continue
                    break
                j += 1
            out.append(s[i:j + 1]); i = j + 1
        elif s.startswith('//', i):
            while i < n and s[i] != '\n': i += 1
        elif s.startswith('/*', i):
            j = s.find('*/', i + 2); i = n if j < 0 else j + 2
        else:
            out.append(c); i += 1
    return ''.join(out)

def split_args(text):
    args, depth, cur, i, n = [], 0, [], 0, len(text)
    while i < n:
        c = text[i]
        if c == "'":
            j = i + 1
            while j < n:
                if text[j] == "'":
                    if j + 1 < n and text[j + 1] == "'":
                        j += 2; continue
                    break
                j += 1
            cur.append(text[i:j + 1]); i = j + 1; continue
        if c in '([': depth += 1
        elif c in ')]': depth -= 1
        if c == ',' and depth == 0:
            args.append(''.join(cur)); cur = []
        else:
            cur.append(c)
        i += 1
    args.append(''.join(cur))
    return [a.strip() for a in args]

def call_args(s, start):
    """s[start] is just after '('; return the text up to the matching ')'."""
    depth, i, n = 1, start, len(s)
    while i < n:
        c = s[i]
        if c == "'":
            j = i + 1
            while j < n:
                if s[j] == "'":
                    if j + 1 < n and s[j + 1] == "'":
                        j += 2; continue
                    break
                j += 1
            i = j + 1; continue
        if c == '(': depth += 1
        elif c == ')':
            depth -= 1
            if depth == 0: return s[start:i]
        i += 1
    return None

STR = re.compile(r"'(?:[^']|'')*'")
IDENT = re.compile(r'"[^"]*"|[A-Za-z_][A-Za-z0-9_]*')
KEEP_CALL = re.compile(r'([A-Za-z_][A-Za-z0-9_]*|"[^"]*")\s*\(')

def normalize(expr):
    """Blank variable names, keep builtin / member names that are followed by '(' or '.', keep literals."""
    expr = re.sub(r'\s+', ' ', expr.strip())
    # peel Format( ... ) with a single argument
    m = re.fullmatch(r'Format\((.*)\)', expr)
    if m and len(split_args(m.group(1))) == 1:
        expr = m.group(1).strip()
    strs = []
    def hide(m):
        strs.append(m.group(0)); return '\x00%d\x00' % (len(strs) - 1)
    t = STR.sub(hide, expr)
    def ident(m):
        w = m.group(0)
        rest = t[m.end():m.end() + 1]
        prev = t[max(0, m.start() - 1):m.start()]
        if rest == '(' or prev == '.' and rest in ('(', ''):
            return w.lower()
        if w.lower() in ('true', 'false', 'not', 'and', 'or', 'xor', 'div', 'mod', 'in'):
            return w.lower()
        if w[0].isdigit():
            return w
        return '_'
    t = IDENT.sub(ident, t)
    t = re.sub(r'(_\.)+', '', t)
    t = re.sub(r'\x00(\d+)\x00', lambda m: strs[int(m.group(1))], t)
    return t.strip()

GENERIC = {'count', 'get', 'add', 'name', 'length', 'contains', 'remove', 'isempty', 'getparent', 'getdocument', 'clear', 'format'}

def interesting(key):
    """A fact is worth comparing when it carries a real input (a non-empty text literal) or a chain of two or more calls."""
    if re.fullmatch(r"[_ .]*", key): return False
    if key.startswith('error:'):
        return len(key) >= 20
    lit = [x for x in STR.findall(key) if len(x) > 2]
    calls = [c.lower() for c in re.findall(r'([a-z_][a-z0-9_]*)\(', key)]
    real = [c for c in calls if c not in GENERIC]
    if lit and real: return True
    if len(real) >= 2: return True
    return False

OBS = re.compile(r'\b(?:Obs|Observer|O|Ob)\.(Observe)\(')
ASSERT = re.compile(r'\bAssert\.(AreEqual|AreNotEqual|IsTrue|IsFalse|ExpectedError|ExpectedErrorCode|AreNearlyEqual)\(')
PROC = re.compile(r'(?m)^\s*procedure\s+("[^"]+"|\w+)\s*\(')
CU = re.compile(r'(?m)^(codeunit)\s+(\d+)\s+("[^"]+"|\w+)')

def tests_of(path):
    raw = open(path, encoding='utf-8', errors='replace').read()
    s = strip_comments(raw)
    # split into codeunits, then into procedures; only [Test] procedures count
    cus = list(CU.finditer(s))
    out = []
    for ci, cm in enumerate(cus):
        body = s[cm.end(): cus[ci + 1].start() if ci + 1 < len(cus) else len(s)]
        procs = list(PROC.finditer(body))
        for pi, pm in enumerate(procs):
            pre = body[max(0, pm.start() - 200):pm.start()]
            if not re.search(r'\[Test\]\s*(\[[^\]]*\]\s*)*$', pre):
                continue
            pb = body[pm.end(): procs[pi + 1].start() if pi + 1 < len(procs) else len(body)]
            keys = set()
            for m in OBS.finditer(pb):
                a = call_args(pb, m.end())
                if a is None: continue
                parts = split_args(a)
                if len(parts) >= 2: keys.add(normalize(parts[1]))
            for m in ASSERT.finditer(pb):
                a = call_args(pb, m.end())
                if a is None: continue
                parts = split_args(a)
                kind = m.group(1)
                if kind in ('AreEqual', 'AreNotEqual', 'AreNearlyEqual') and len(parts) >= 2: keys.add(normalize(parts[1]))
                elif kind in ('IsTrue', 'IsFalse') and parts: keys.add(normalize(parts[0]))
                elif kind.startswith('ExpectedError') and parts: keys.add('error:' + normalize(parts[0]))
            keys = {k for k in keys if interesting(k)}
            out.append((cm.group(2), cm.group(3), pm.group(1), keys))
    return out

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--corpus', required=True)
    ap.add_argument('--new', action='append', default=[])
    ap.add_argument('--new-git')
    ap.add_argument('--min-keys', type=int, default=3)
    ap.add_argument('--min-share', type=float, default=0.6)
    ap.add_argument('--show', action='store_true')
    a = ap.parse_args()
    root = a.corpus
    files = []
    for app in ('tests/al-language', 'tests/al-language-onprem'):
        for dp, _, fs in os.walk(os.path.join(root, app)):
            for f in fs:
                if f.endswith('.al'): files.append(os.path.join(dp, f))
    if a.new_git:
        import subprocess
        added = subprocess.run(['git', '-C', root, 'diff', '--name-only', '--diff-filter=A', a.new_git + '...HEAD'], capture_output=True, text=True).stdout.split()
        a.new += [f for f in added if f.endswith('.al')]
    newroots = [os.path.abspath(p if os.path.isabs(p) else os.path.join(root, p)) for p in a.new]
    def is_new(f):
        f = os.path.abspath(f)
        return any(f == r or f.startswith(r + os.sep) for r in newroots)
    index = collections.defaultdict(list)   # key -> [(file, cu, proc)]
    tests = []
    for f in sorted(files):
        for cu, name, proc, keys in tests_of(f):
            rel = os.path.relpath(f, root)
            tests.append((rel, is_new(f), cu, name, proc, keys))
            for k in keys: index[k].append((rel, cu, proc))
    reported = 0
    total_new = 0
    dup_facts = 0
    for rel, new, cu, name, proc, keys in tests:
        if a.new and not new: continue
        total_new += 1
        if len(keys) < a.min_keys: continue
        hits = {k: [p for p in index[k] if p[0] != rel] for k in keys}
        dups = {k: v for k, v in hits.items() if v}
        share = len(dups) / len(keys)
        if share >= a.min_share:
            reported += 1; dup_facts += len(dups)
            print('%s  %s.%s: %d of %d facts exist elsewhere (%.0f%%)' % (rel, name, proc, len(dups), len(keys), share * 100))
            if a.show:
                for k, v in sorted(dups.items())[:12]:
                    print('    %s' % k[:110]); print('        also in %s %s.%s' % (v[0][0], v[0][1], v[0][2]))
    print('\n%d tests looked at, %d with %d%% or more of their facts already tested elsewhere (%d facts)' % (total_new, reported, a.min_share * 100, dup_facts))

if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""usage: convert.py <observe.xml> <dump.xml|-> <al file>...  (turns Observer.Finish() into Expect/Verify using the observed values)"""
import sys, re, xml.etree.ElementTree as ET
RS, US = '␞', '␟'

def messages(path):
    out = {}
    for tc in ET.parse(path).iter('testcase'):
        f = tc.find('failure')
        if f is None: f = tc.find('error')
        if f is None: continue
        t = (f.text or f.get('message') or '').split('\nAL Callstack')[0].split('\n\nCall stack')[0].strip('\n')
        out[(tc.get('classname'), tc.get('name'))] = t
    return out

def dump_entries(path):
    if path == '-': return {}
    chunks = []
    for tc in ET.parse(path).iter('testcase'):
        f = tc.find('failure')
        if f is None: f = tc.find('error')
        if f is None: continue
        t = (f.text or f.get('message') or '').split('\nAL Callstack')[0].split('\n\nCall stack')[0].strip('\n')
        chunks.append((tc.get('name'), t[len('DUMP:'):] if t.startswith('DUMP:') else t))
    chunks.sort()
    full = ''.join(c for _, c in chunks if c != '<empty>')
    entries = {}
    for m in re.finditer(r'@@ENTRY (\d+)@@', full):
        pass
    parts = re.split(r'@@ENTRY (\d+)@@', full)
    for i in range(1, len(parts), 2):
        entries[int(parts[i])] = parts[i + 1]
    return entries

def lit(v):
    return "'" + v.replace("'", "''") + "'"

import os
DISCARD = set(l.strip() for l in open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'discard.txt')) if l.strip()) if os.path.exists(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'discard.txt')) else set()

def is_dump(k, v):
    import re as _re
    if '<BCReportInformation' in v or '<?xml' in v and len(v) > 600: return True
    if len(_re.findall(r'(?:^|\| )\d+:[^|=]+=', v)) >= 4: return True
    if len(v) > 1500: return True
    if _re.search(r'[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}', v) and 'Package' in v: return True
    if k.startswith('fieldcount') or k.endswith('_fieldcount'): return True
    return False

def main():
    obs = messages(sys.argv[1]); dump = dump_entries(sys.argv[2])
    pairs = {}
    problems = []
    for key, t in obs.items():
        m = re.match(r'OBSERVE(?:#\d+)?:(.*)$', t, re.S)
        if not m: continue
        body = m.group(1)
        k = 1
        while body.endswith('<MORE>'):
            nxt = obs.get((key[0], '%s_More%d' % (key[1], k)))
            body = body[:-len('<MORE>')]
            if nxt is None or not nxt.startswith('OBSERVEMORE:'):
                problems.append('missing continuation %d of %s %s' % (k, key[0], key[1])); break
            body += nxt[len('OBSERVEMORE:'):]
            k += 1
        pairs[key] = body
    for alp in sys.argv[3:]:
        src = open(alp).read().split('\n')
        out = []; cu = None; proc = None; done = 0
        for line in src:
            m = re.match(r'^\s*codeunit\s+(\d+)\s', line)
            if m: cu = m.group(1)
            m = re.match(r'^\s*(?:local\s+)?procedure\s+(\w+)\s*\(', line)
            if m: proc = m.group(1)
            m = re.match(r'^(\s*)(\w+)\.Finish\(\);\s*$', line)
            if m and cu and proc:
                ind, var = m.groups()
                key = ('Codeunit ' + cu, proc)
                if 'EnvData' in proc or proc in DISCARD:
                    out.append('%s%s.Discard(); // tier dependent: recorded in the manifest, not asserted' % (ind, var)); done += 1; continue
                if key not in pairs:
                    problems.append('no observation for %s' % (key,)); out.append(line); continue
                ignored = []
                for ent in pairs[key].split(RS):
                    if not ent: continue
                    k, _, v = ent.partition(US)
                    if is_dump(k, v):
                        if k not in ignored: ignored.append(k)
                        open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'not-asserted.tsv'), 'a').write('%s\t%s\t%s\t%s\n' % (alp, proc, k, v.replace('\n', ' ')))
                        continue
                    out.append("%s%s.Expect(%s, %s);" % (ind, var, lit(k), lit(v)))
                for k in ignored:
                    out.append("%s%s.Ignore(%s); // a dump or environment data: observed, not asserted" % (ind, var, lit(k)))
                out.append('%s%s.Verify();' % (ind, var)); done += 1; continue
            out.append(line)
        text = '\n'.join(out)
        text = re.sub(r"\n    \[Test\]\n    procedure \w+_More\d+\(\)\n    var\n        Observer: Codeunit \"Probe Observer\";\n    begin\n        Observer.More\(\);\n    end;\n", "\n", text)
        open(alp, 'w').write(text)
        print('%s: %d finished' % (alp, done))
    for p in problems: print('PROBLEM', p)

main()

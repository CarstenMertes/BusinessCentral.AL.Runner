#!/usr/bin/env python3
"""Survey of AL objects the corpus references but does not define (ADR-013).

usage: tools/external_survey.py <root> <junit.xml> [--members N]

<junit.xml> is the file written by `al-test-cli --root . --junit <file>`.
Prints, per external object, the number of tests whose *first* blocker is
"<kind> `X` is outside the workspace", the number of test codeunits involved,
and the members the corpus calls on it. Member extraction is textual (variable
declarations of the object type, then `Var.Member`), so it is a lower bound for
tables reached through `Rec`, `RecordRef` or `Database::`.
"""
import collections, os, re, sys, xml.etree.ElementTree as ET

root, junit = sys.argv[1], sys.argv[2]
top = int(sys.argv[sys.argv.index('--members') + 1]) if '--members' in sys.argv else 40

blocked, cus = collections.Counter(), collections.defaultdict(set)
for tc in ET.parse(junit).getroot().iter('testcase'):
    s = tc.find('skipped')
    if s is None:
        continue
    m = re.search(r'(table|codeunit|page|report|xmlport|enum|query|interface) `([^`]+)` is outside the workspace', s.get('message'))
    if m:
        blocked[(m[1], m[2])] += 1
        cus[(m[1], m[2])].add(tc.get('classname'))

files = []
for dp, _, fs in os.walk(os.path.join(root, 'corpus', 'tests')):
    for f in fs:
        if f.endswith('.al'):
            t = open(os.path.join(dp, f), encoding='utf-8-sig').read()
            files.append(re.sub(r"//[^\n]*", '', t))
src = '\n'.join(files)

RECORD_METHODS = set('''get insert modify delete deleteall init reset setrange setfilter setcurrentkey findfirst findlast findset
find next count isempty calcfields calcsums copy copyfilters testfield validate rename modifyall setrecfilter getfilter
getrangemin getrangemax fieldno fieldname fieldcaption tablename tablecaption recordid hasfilter getview setview
transferfields setrecfilter lockTable getposition setposition readpermission writepermission mark markedonly clearmarks
addlink getlinks deletelinks copylinks fieldactive currentkey isTemporary'''.lower().split())

def members(kind, name):
    q = re.escape(name)
    decl = re.compile(r'(\w+)\s*:\s*(?:temporary\s+)?' + kind + r'\s+"?' + q + r'"?\b', re.I)
    vars_ = set()
    flds, meths = collections.Counter(), collections.Counter()
    for text in files:  # variables are matched within the file that declares them
        fv = {m[1] for m in decl.finditer(text)}
        vars_ |= fv
        for v in fv:
            for m in re.finditer(r'\b' + re.escape(v) + r'\.("[^"]+"|\w+)(\s*\()?', text, re.I):
                mem = m[1].strip('"')
                (meths if (m[2] or mem.lower() in RECORD_METHODS) else flds)[mem] += 1
    static = len(re.findall(kind + r'::"?' + q + r'"?\b', src, re.I))
    dbref = len(re.findall(r'Database::"?' + q + r'"?\b', src, re.I)) if kind.lower() == 'record' else 0
    return len(vars_), meths, flds, static, dbref

print(f'{"tests":>5} {"cus":>4}  object')
for (kind, name), n in blocked.most_common(top):
    k = {'table': 'Record', 'codeunit': 'Codeunit', 'page': 'Page', 'report': 'Report', 'xmlport': 'Xmlport',
         'enum': 'Enum', 'query': 'Query', 'interface': 'Interface'}[kind]
    nv, meths, flds, static, dbref = members(k, name)
    desc = f'vars={nv}'
    if static: desc += f' static={static}'
    if dbref: desc += f' Database::={dbref}'
    print(f'{n:5} {len(cus[(kind, name)]):4}  {kind} "{name}"  [{desc}]')
    if meths: print('            methods: ' + ', '.join(f'{a}({b})' for a, b in meths.most_common(12)))
    if flds: print('            fields:  ' + ', '.join(f'{a}({b})' for a, b in flds.most_common(12)))
print()
tot = collections.Counter()
for (kind, _), n in blocked.items(): tot[kind] += n
print('totals by kind:', dict(tot), 'objects:', len(blocked), 'tests:', sum(tot.values()))

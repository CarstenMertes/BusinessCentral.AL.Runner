import os, re, sys, json, collections, xml.etree.ElementTree as ET
root, junit = sys.argv[1], sys.argv[2]
OBJ = re.compile(r'^\s*(codeunit|table|page|enum|interface|query|report|xmlport|tableextension|pageextension|enumextension|reportextension|permissionset|permissionsetextension|profile)\s+(\d+)?\s*("[^"]+"|\w+)', re.I|re.M)
REF = re.compile(r':\s*(?:temporary\s+)?(Record|Codeunit|Page|Report|Query|XmlPort|Enum|TestPage|Interface)\s+("[^"]+"|[A-Za-z_]\w*)', re.I)
defined = set(); files = {}
for dp, _, fs in os.walk(root):
    for f in fs:
        if f.endswith('.al'):
            p = os.path.join(dp, f); src = open(p, encoding='utf-8-sig').read(); src = re.sub(r"//[^\n]*|'(?:[^']|'')*'", '', src); files[p] = src
            for m in OBJ.finditer(src): defined.add(m.group(3).strip('"').lower())
area_of = lambda p: os.path.relpath(p, root).split(os.sep)[0]
res = collections.defaultdict(lambda: dict(files=0, test_cus=set(), tests=0, handlers=collections.Counter(), ext=collections.Counter(), testpage=0, fixtures=collections.Counter()))
cu_area = {}
for p, src in files.items():
    a = area_of(p); r = res[a]; r['files'] += 1
    for m in re.finditer(r'^\s*codeunit\s+(\d+)', src, re.I|re.M):
        if re.search(r'Subtype\s*=\s*Test', src): r['test_cus'].add(int(m.group(1))); cu_area[int(m.group(1))] = a
    r['tests'] += len(re.findall(r'\[Test\]', src))
    for h in re.findall(r'\[(\w+Handler)\]', src): r['handlers'][h] += 1
    r['testpage'] += len(re.findall(r'\bTestPage\b', src))
    for kind, name in REF.findall(src):
        n = name.strip('"')
        if kind.lower() in ('testpage',): continue
        if n.lower() in defined: 
            if n.startswith('ALT') or n.startswith('IALT'): r['fixtures'][n] += 1
        elif n.lower() not in ('temporary',):
            r['ext'][f'{kind} {n}'] += 1
# junit
tree = ET.parse(junit); jr = collections.defaultdict(lambda: [0,0])
oracle_cus = set()
for ts in tree.getroot().iter('testsuite'):
    cu = int(ts.get('name').split()[-1]); oracle_cus.add(cu)
    a = cu_area.get(cu, '?'); jr[a][0] += int(ts.get('tests')); jr[a][1] += int(ts.get('failures')) + int(ts.get('errors'))
out = {}
for a in sorted(res):
    r = res[a]
    out[a] = dict(files=r['files'], test_codeunits=len(r['test_cus']), test_attrs=r['tests'], oracle_tests=jr[a][0], oracle_fail=jr[a][1],
                  handlers=dict(r['handlers']), testpage_refs=r['testpage'], fixtures=dict(r['fixtures'].most_common(8)),
                  external_refs=dict(r['ext'].most_common(10)))
out['?'] = dict(oracle_tests=jr['?'][0])
json.dump(out, sys.stdout, indent=1)

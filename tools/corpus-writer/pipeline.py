#!/usr/bin/env python3
"""usage: pipeline.py <writer dir name> <fixes script|-> <codeunit id regex> [--verify-only]
copies the writer's files into the corpus app, applies the fixes, normalizes to the observation form, runs it, converts and verifies."""
import sys, os, subprocess, shutil, re, glob, xml.etree.ElementTree as ET
WORK = os.environ.get('WORK', os.path.expanduser('~/.cache/corpus-writer'))
CORPUS = os.environ.get('CORPUS', os.path.expanduser('~/corpus'))
TOOLS = os.path.dirname(os.path.abspath(__file__))
os.makedirs(WORK, exist_ok=True)
C = CORPUS + '/tests/al-language'
STAGE = os.environ.get('DRAFTS', WORK + '/drafts')
W = WORK
writer, fixes, ids = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(W + '/xml', exist_ok=True)
os.chdir(C)
files = []
for root, _, fs in os.walk(os.path.join(STAGE, writer)):
    for f in fs:
        if f.endswith('.al'):
            rel = os.path.relpath(os.path.join(root, f), os.path.join(STAGE, writer))
            os.makedirs(os.path.dirname(rel) or '.', exist_ok=True)
            shutil.copy(os.path.join(root, f), rel)
            files.append(rel)
subprocess.check_call(['python3', TOOLS + '/obsmode.py', 'on'])
if fixes != '-':
    subprocess.check_call(['python3', fixes])
files = [f for f in files if os.path.exists(f)]
subprocess.check_call(['python3', TOOLS + '/normalize.py'] + files)
def run(tag):
    out = subprocess.run([TOOLS + '/run-corpus.sh', ids], capture_output=True, text=True, timeout=1500).stdout
    print(out.strip().split('\n')[0][:200])
    shutil.copy(W + '/cout.xml', W + '/xml/%s-%s.xml' % (writer, tag))
    return W + '/xml/%s-%s.xml' % (writer, tag)
import time
for rnd in range(10):
    obs = run('observe')
    nm = subprocess.run(['python3', TOOLS + '/needmore.py', obs], capture_output=True, text=True).stdout.strip()
    print('needmore round', rnd, ':', nm.replace('\n', ' ; ')[:600])
    if not nm: break
    for line in nm.split('\n'):
        cu, name, k = line.split()
        want = int(k)
        for f in files:
            t = open(f).read()
            if 'procedure %s(' % name in t and re.search(r'(?m)^codeunit %s ' % cu, t) and t.index('codeunit %s ' % cu) < t.index('procedure %s(' % name):
                t = re.sub(r"\n    \[Test\]\n    procedure %s_More\d+\(\)\n    var\n        Observer: Codeunit \"Probe Observer\";\n    begin\n        Observer.More\(\);\n    end;\n" % re.escape(name), "\n", t)
                open(f, 'w').write(t)
                subprocess.check_call(['python3', TOOLS + '/addmore.py', f, name, str(want + 3)])
                break
print(subprocess.run(['python3', TOOLS + '/convert.py', obs, '-'] + files, capture_output=True, text=True).stdout)
subprocess.check_call(['python3', TOOLS + '/obsmode.py', 'off'])
print(subprocess.run(['python3', TOOLS + '/dropempty.py'] + files, capture_output=True, text=True).stdout)
ver = run('verify')
bad = []
for tc in ET.parse(ver).iter('testcase'):
    f = tc.find('failure')
    if f is None: f = tc.find('error')
    if f is not None:
        bad.append((tc.get('classname'), tc.get('name'), (f.text or '')[:200].replace('\n', ' ')))
for b in bad: print('FAIL', b)
print(len(bad), 'failing')

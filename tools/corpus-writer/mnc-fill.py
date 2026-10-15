#!/usr/bin/env python3
"""usage: mnc-fill.py <scratch.al> <dest case path> [--until N] : runs the checker (AL 16, 27.0 symbols) on a scratch case with a placeholder expect, writes the case with the found diagnostics as @expect lines"""
import os, sys, subprocess, tempfile, shutil, os, re
src, dest = sys.argv[1], sys.argv[2]
extra = []
if '--until' in sys.argv: extra.append('// @until %s' % sys.argv[sys.argv.index('--until') + 1])
if '--from' in sys.argv: extra.append('// @from %s' % sys.argv[sys.argv.index('--from') + 1])
body = [l for l in open(src).read().split('\n')]
body = [l for l in body if not l.startswith('// @expect')]
with tempfile.TemporaryDirectory() as d:
    open(os.path.join(d, 'x.al'), 'w').write('// @expect AL0000 | x\n' + '\n'.join(body))
    out = subprocess.run(['python3', os.path.expanduser(os.environ.get('CORPUS','~/corpus')) + '/.github/scripts/check-must-not-compile.py', '--al', os.environ['AL'], '--symbols', os.environ['SYMBOLS'], '--al-major', '16', '--cases', d], capture_output=True, text=True).stdout
found = re.findall(r'unexpected diagnostic (AL\d+): (.*)', out)
if not found:
    print('NO DIAGNOSTICS', out); sys.exit(1)
found.sort()
head = ['// @expect %s | %s' % (c, m) for c, m in found] + extra
open(dest, 'w').write('\n'.join(head) + '\n' + '\n'.join(body))
print(dest, len(found), 'diagnostics')

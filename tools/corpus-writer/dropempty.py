#!/usr/bin/env python3
"""usage: dropempty.py <al files>  removes [Test] procedures that ignore everything (no Expect line left)"""
import sys, re
for p in sys.argv[1:]:
    s = open(p).read()
    parts = re.split(r'(?m)^(?=    \[Test\]\n)', s)
    out = []; dropped = []
    for part in parts:
        if part.startswith('    [Test]\n') and '.Ignore(' in part and '.Expect(' not in part:
            m = re.search(r'procedure (\w+)\(', part)
            dropped.append(m.group(1))
            # keep what follows the test inside the same chunk (next non-test members): cut at the first line after 'end;' block
            k = part.index('\n    end;\n') + len('\n    end;\n')
            out.append(part[k:])
        else:
            out.append(part)
    if dropped:
        open(p, 'w').write(''.join(out))
        print(p, 'dropped', ' '.join(dropped))

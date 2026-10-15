#!/usr/bin/env python3
"""usage: addmore.py <al file> <test> <n>   adds n continuation tests after <test>"""
import sys
path, test, n = sys.argv[1], sys.argv[2], int(sys.argv[3])
s = open(path).read()
i = s.index('procedure %s(' % test)
j = s.index('\n    end;\n', i) + len('\n    end;\n')
add = ''
for k in range(1, n + 1):
    add += '\n    [Test]\n    procedure %s_More%d()\n    var\n        Observer: Codeunit "Probe Observer";\n    begin\n        Observer.More();\n    end;\n' % (test, k)
open(path, 'w').write(s[:j] + add + s[j:])

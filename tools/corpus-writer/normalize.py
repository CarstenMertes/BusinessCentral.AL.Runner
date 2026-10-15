#!/usr/bin/env python3
"""usage: normalize.py <al file>...   removes Observer.Expect lines and turns Observer.Verify() back into Observer.Finish() (observation form)"""
import sys, re
for p in sys.argv[1:]:
    s = open(p).read()
    s = re.sub(r"^[ \t]*\w+\.Expect\(.*\);[ \t]*\n", "", s, flags=re.M)
    s = re.sub(r"(\w+)\.Verify\(\);", r"\1.Finish();", s)
    open(p, 'w').write(s)

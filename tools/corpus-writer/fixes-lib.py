import re, os
def rd(p): return open(p).read()
def wr(p, s): open(p, 'w').write(s)
def drop_test(path, name):
    s = rd(path)
    pat = re.compile(r'    \[Test\]\n(?:    \[[^\n]*\]\n)*    procedure ' + re.escape(name) + r'\(\).*?\n    end;\n\n?', re.S)
    s2, n = pat.subn('', s, count=1)
    assert n == 1, (path, name)
    wr(path, s2)
def sub(path, old, new, count=1):
    s = rd(path)
    assert old in s, (path, old[:70])
    wr(path, s.replace(old, new, count) if count else s.replace(old, new))
def noerr(path, stmt):
    s = rd(path)
    pat = re.compile(r"( *)asserterror " + re.escape(stmt) + r"\n *O(?:bserver)?\.ObserveError\('([^']*)'\);")
    m = pat.search(s)
    assert m, (path, stmt)
    obs = 'Observer' if 'Observer.ObserveError' in m.group(0) else 'O'
    s = s[:m.start()] + "%s%s\n%s%s.Observe('%s', 'no error');" % (m.group(1), stmt, m.group(1), obs, m.group(2)) + s[m.end():]
    wr(path, s)

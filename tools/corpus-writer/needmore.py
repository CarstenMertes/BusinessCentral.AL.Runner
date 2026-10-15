#!/usr/bin/env python3
"""usage: needmore.py <observe.xml>  -> prints 'codeunit test pieces_missing' for every truncated observation"""
import sys, re, xml.etree.ElementTree as ET
obs = {}
for tc in ET.parse(sys.argv[1]).iter('testcase'):
    f = tc.find('failure')
    if f is None: f = tc.find('error')
    if f is None: continue
    obs[(tc.get('classname'), tc.get('name'))] = (f.text or f.get('message') or '').split('\n\nCall stack')[0]
for (cu, name), t in obs.items():
    if t.startswith('OBSERVE') and not t.startswith('OBSERVEMORE') and t.endswith('<MORE>'):
        k = 1
        body = t
        while body.endswith('<MORE>'):
            nxt = obs.get((cu, '%s_More%d' % (name, k)))
            if nxt is None: break
            body = nxt
            k += 1
        if body.endswith('<MORE>') or (cu, '%s_More%d' % (name, k - 1)) not in obs and k == 1:
            print(cu.split()[-1], name, k)

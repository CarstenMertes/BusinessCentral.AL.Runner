#!/usr/bin/env python3
"""usage: failures.py <run id>  -> failing tests per version (downloads junit artifacts)"""
import sys,subprocess,glob,os,shutil,collections
import xml.etree.ElementTree as ET
run=sys.argv[1]
d=os.path.expanduser(os.environ.get('WORK','~/.cache/corpus-writer'))+'/ci/run-'+run
shutil.rmtree(d,ignore_errors=True); os.makedirs(d)
subprocess.run(['gh','run','download',run,'-R','aldevtools/BusinessCentral.AL.Language.Tests','-p','junit-test-results-*','-D',d],capture_output=True)
res=collections.defaultdict(dict)
for dd in sorted(glob.glob(d+'/junit-test-results-*')):
    v=dd.split('-')[-1]
    for p in glob.glob(dd+'/*.xml'):
        for tc in ET.parse(p).iter('testcase'):
            f=tc.find('failure')
            if f is None: f=tc.find('error')
            if f is None: continue
            res[(tc.get('classname'),tc.get('name'))][v]=((f.get('message') or '')+' '+(f.text or ''))[:int(sys.argv[2]) if len(sys.argv)>2 else 260].replace('\n',' ')
for k,v in sorted(res.items()):
    print(k,sorted(v)); print('    ',list(v.values())[0])
print(len(res),'failing tests')

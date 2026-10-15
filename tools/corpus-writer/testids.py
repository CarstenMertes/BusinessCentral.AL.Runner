#!/usr/bin/env python3
import re,glob,sys
ids=[]
for f in glob.glob(sys.argv[1]+'/**/*.al',recursive=True):
    s=open(f).read()
    parts=re.split(r'(?m)^(?=(?:table|page|report|query|xmlport|codeunit|enum|pageextension|tableextension|reportextension|interface|permissionset|controladdin)\s+\d+\s|interface\s+")',s)
    for p in parts:
        m=re.match(r'codeunit\s+(\d+)\s',p)
        if m and re.search(r'Subtype\s*=\s*Test',p): ids.append(m.group(1))
print('|'.join(sorted(set(ids),key=int)))

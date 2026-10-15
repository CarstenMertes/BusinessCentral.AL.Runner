#!/usr/bin/env python3
"""obsmode.py on|off : on = observing variant of the helper (SingleInstance state keeps the rest between tests); off = the published form"""
import sys, shutil
import os, subprocess
CORPUS = os.environ.get('CORPUS', os.path.expanduser('~/corpus'))
P = CORPUS + '/tests/al-language/_fixtures/ProbeObserver.Codeunit.al'
if sys.argv[1] == 'off':
    # the published form is the committed file: commit changes of the helper BEFORE observing
    subprocess.check_call(['git', '-C', CORPUS, 'checkout', '--', 'tests/al-language/_fixtures/ProbeObserver.Codeunit.al']); sys.exit()
s = open(P).read()
if 'SingleInstance = true' in s: sys.exit()
a = s.index("    procedure Finish()")
b = s.index("// Kept from an earlier")
new = '''    procedure Finish()
    var
        State: Codeunit "Probe Observer State";
    begin
        State.Set(Pairs);
        Pairs := '';
        State.Raise('OBSERVE');
    end;

    procedure More()
    var
        State: Codeunit "Probe Observer State";
    begin
        State.Raise('OBSERVEMORE');
    end;
}

codeunit 69163 "Probe Observer State"
{
    SingleInstance = true;

    var
        Rest: Text;

    procedure Set(NewRest: Text)
    begin
        Rest := NewRest;
    end;

    procedure Raise(Prefix: Text)
    var
        Piece: Text;
    begin
        Piece := CopyStr(Rest, 1, 1800);
        Rest := CopyStr(Rest, 1801);
        if Rest <> '' then
            Error('%1:%2<MORE>', Prefix, Piece);
        Error('%1:%2', Prefix, Piece);
    end;
}

'''
open(P, 'w').write(s[:a] + new + s[b:])

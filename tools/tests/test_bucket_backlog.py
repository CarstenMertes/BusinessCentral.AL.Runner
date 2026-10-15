#!/usr/bin/env python3
"""Test of tools/bucket_backlog.py on hand-made junit files: python3 tools/tests/test_bucket_backlog.py"""
import json, os, subprocess, sys, tempfile, unittest

TOOL = os.path.join(os.path.dirname(__file__), "..", "bucket_backlog.py")


def junit(cases):
    s = '<?xml version="1.0"?>\n<testsuites>\n'
    by = {}
    for cu, name, kind, msg, t in cases:
        by.setdefault(cu, []).append((name, kind, msg, t))
    for cu, tests in by.items():
        s += f'<testsuite name="CU{cu}"><properties><property name="codeunit" value="{cu}" /></properties>\n'
        for name, kind, msg, t in tests:
            head = f'<testcase classname="CU{cu}" name="{name}" time="{t}"'
            if kind == "pass":
                s += head + " />\n"
            elif kind == "skip":
                s += head + f'><skipped message="{msg}" /></testcase>\n'
            else:
                s += head + f'><failure message="{msg}" /></testcase>\n'
        s += "</testsuite>\n"
    return s + "</testsuites>\n"


class T(unittest.TestCase):
    def run_tool(self, d, *extra):
        subprocess.run([sys.executable, TOOL, d, "--zip-dir", "/z", *extra], check=True, capture_output=True)
        return json.load(open(os.path.join(d, "backlog.json"))), open(os.path.join(d, "backlog.md")).read()

    def test_groups_folds_ranks_and_diffs(self):
        with tempfile.TemporaryDirectory() as t:
            cur, prev = os.path.join(t, "cur"), os.path.join(t, "prev")
            os.mkdir(cur), os.mkdir(prev)
            open(os.path.join(cur, "Bank.xml"), "w").write(junit([
                (100, "A", "skip", "Codeunit X.Run: builtin 'Foo' not supported (3)", 0.1),
                (100, "B", "skip", "Codeunit X.Other: builtin 'Bar' not supported (7)", 0.2),
                (101, "C", "skip", "builtin 'Baz' not supported (9)", 0.3),
                (101, "D", "fail", "Expected 5 but was 6", 9.5),
                (101, "E", "pass", "", 0.0),
                (101, "F", "pass", "", 0.0),
            ]))
            open(os.path.join(cur, "VAT.xml"), "w").write(junit([(200, "G", "skip", "builtin 'Q' not supported (1)", 0.0)]))
            open(os.path.join(prev, "Bank.xml"), "w").write(junit([
                (101, "E", "pass", "", 0.0), (101, "F", "fail", "x", 0.0), (101, "D", "pass", "", 0.0)]))
            open(os.path.join(cur, "buckets.tsv"), "w").write("bucket\texit\tseconds\nBank\t7\t12\nVAT\t7\t3\n")
            j, md = self.run_tool(cur, "--previous", prev)
            top = j["items"][0]
            self.assertEqual(top["reason"], "builtin <name> not supported (N)")
            self.assertEqual(top["tests"], 4)
            bank = [b for b in top["buckets"] if b["bucket"] == "Bank"][0]
            self.assertEqual(bank["codeunits"], [100, 101])
            self.assertEqual(bank["rerun"], "alrunner test /z/Tests-Bank.Source.zip --codeunit 100 --codeunit 101")
            self.assertEqual(j["items"][1]["kind"], "failed")
            self.assertEqual(j["buckets"]["Bank"]["exit"], 7)
            self.assertEqual([x["test"] for x in j["lost_passes"]], ["D"])
            self.assertEqual(j["new_passes"], {"Bank": 1})
            self.assertEqual(j["slowest"][0]["test"], "D")
            self.assertIn("## Lost passes (1)", md)


if __name__ == "__main__":
    unittest.main()

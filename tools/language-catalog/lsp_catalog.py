"""Generates docs/language-catalog.json by asking the AL language server, the way VS Code does.

For each BC major it starts the language server of that major's AL Language extension on a probe
workspace (app.json with the major's runtime, the platform symbols of the major's System.app),
opens small probe files (probes.py) and records what the server answers: completion lists of data
types, members per type, statement keywords, attributes, trigger events, triggers and properties
per object kind and property values; signature help for every method overload. A Cloud-target run
of the same workspace marks the methods only an OnPrem target offers.

Nothing of Microsoft's is written to the repository: only names and signatures. Facts the server
does not give (the runtime version an item exists from, deprecation dates, property value types)
are left out. Usage: see generate.sh.
"""

import json
import os
import pathlib
import shutil
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from lsp import LspClient  # noqa: E402
import probes  # noqa: E402
from probes import CURSOR  # noqa: E402

KIND_METHOD, KIND_FUNCTION, KIND_PROPERTY, KIND_KEYWORD = 2, 3, 10, 14


def split_top(s, sep):
    out, depth, cur = [], 0, ""
    for ch in s:
        if ch == "[":
            depth += 1
        elif ch == "]":
            depth -= 1
        if ch == sep and depth == 0:
            out.append(cur)
            cur = ""
        else:
            cur += ch
    out.append(cur)
    return [x.strip() for x in out if x.strip()]


def parse_signature(label):
    """`procedure Name(var A: T; [B: U]; [C: V, ...]): R` -> (name, params, returns).
    A param is (name, type, by_var, optional, variadic)."""
    s = label.strip()
    for kw in ("procedure ", "trigger "):
        if s.startswith(kw):
            s = s[len(kw):]
    open_i = s.find("(")
    if open_i < 0:
        return s, [], ""
    name = s[:open_i].strip()
    depth, close_i = 0, len(s) - 1
    for i in range(open_i, len(s)):
        if s[i] == "(":
            depth += 1
        elif s[i] == ")":
            depth -= 1
            if depth == 0:
                close_i = i
                break
    inner = s[open_i + 1:close_i]
    rest = s[close_i + 1:].strip()
    ret = rest[1:].strip() if rest.startswith(":") else ""
    params = []
    # BC 28 and later separate parameters with `;`, BC 27 with `,`.
    sep = ";" if len(split_top(inner, ";")) > 1 or ";" in inner else ","
    parts = []
    for p in split_top(inner, sep):
        # BC 27 writes a repeated parameter as `Content: Joker, ...`: the `...` belongs to the one before.
        if p == "..." and parts:
            parts[-1] += ", ..."
        else:
            parts.append(p)
    for p in parts:
        by_var = p.startswith("var ")
        if by_var:
            p = p[4:].strip()
        optional = p.startswith("[") and p.endswith("]")
        if optional:
            p = p[1:-1].strip()
        variadic = p.endswith(", ...") or p.endswith(",...")
        if variadic:
            p = p.rsplit(",", 1)[0].strip()
        if not by_var and p.startswith("var "):
            by_var = True
            p = p[4:]
        pname, _, ptype = p.partition(":")
        params.append((pname.strip(), ptype.strip(), by_var, optional, variadic))
    return name, params, ret


def sig_text(params):
    parts, named = [], []
    for name, ty, by_var, optional, variadic in params:
        t = ("var " if by_var else "") + ty + ("..." if variadic else "")
        d = f"{'var ' if by_var else ''}{name}: {ty}{'...' if variadic else ''}"
        parts.append(f"[{t}]" if optional else t)
        named.append(f"[{d}]" if optional else d)
    return "(" + ", ".join(parts) + ")", "; ".join(named)


class Server:
    def __init__(self, host_cmd, ws):
        self.ws = ws
        self.client = LspClient(host_cmd, str(ws))
        self.version = 0
        self.file = ws / "Probe.al"
        uri = ws.as_uri()
        self.client.request(
            "initialize",
            {
                "processId": os.getpid(),
                "rootUri": uri,
                "rootPath": str(ws),
                "workspaceFolders": [{"uri": uri, "name": ws.name}],
                "capabilities": {
                    "textDocument": {
                        "completion": {"completionItem": {"snippetSupport": True, "tagSupport": {"valueSet": [1]}}},
                        "signatureHelp": {"signatureInformation": {"parameterInformation": {"labelOffsetSupport": True}}},
                    },
                    "workspace": {"configuration": True, "workspaceFolders": True},
                },
            },
            timeout=300,
        )
        self.client.notify("initialized", {})
        self.client.notify(
            "workspace/didChangeConfiguration",
            {
                "settings": {
                    "workspacePath": str(ws),
                    "alResourceConfigurationSettings": {
                        "assemblyProbingPaths": ["./.netpackages"],
                        "codeAnalyzers": [],
                        "enableCodeAnalysis": False,
                        "backgroundCodeAnalysis": "None",
                        "packageCachePaths": ["./.alpackages"],
                        "ruleSetPath": None,
                        "enableCodeActions": False,
                        "incrementalBuild": False,
                        "outputAnalyzerStatistics": False,
                        "enableExternalRulesets": False,
                    },
                    "setActiveWorkspace": True,
                    "expectedProjectReferenceDefinitions": [],
                    "activeWorkspaceClosure": [str(ws)],
                }
            },
        )
        for name, lang in (("app.json", "json"), ("Objects.al", "al")):
            f = ws / name
            self.client.notify(
                "textDocument/didOpen",
                {"textDocument": {"uri": f.as_uri(), "languageId": lang, "version": 1, "text": f.read_text()}},
            )
        try:
            self.client.request(
                "al/setActiveWorkspace",
                {
                    "currentWorkspaceFolderPath": {"uri": uri, "name": ws.name, "index": 0},
                    "settings": {"workspacePath": str(ws), "setActiveWorkspace": True},
                },
                timeout=120,
            )
        except Exception:
            pass
        # Wait until the project is loaded: the probe record's members are known.
        for _ in range(60):
            if any(i["label"] == "Insert" for i in self.complete(probes.member_probe('Record "Probe Table"', "X." + CURSOR))):
                return
            time.sleep(2)
        raise RuntimeError(f"the language server in {ws} did not load the probe project")

    def _position(self, text):
        at = text.index(CURSOR)
        before = text[:at]
        text = text.replace(CURSOR, "")
        line = before.count("\n")
        col = len(before) - (before.rfind("\n") + 1)
        self.version += 1
        if self.version == 1:
            self.client.notify(
                "textDocument/didOpen",
                {"textDocument": {"uri": self.file.as_uri(), "languageId": "al", "version": 1, "text": text}},
            )
        else:
            self.client.notify(
                "textDocument/didChange",
                {"textDocument": {"uri": self.file.as_uri(), "version": self.version}, "contentChanges": [{"text": text}]},
            )
        return {"textDocument": {"uri": self.file.as_uri()}, "position": {"line": line, "character": col}}

    def complete(self, text):
        r = self.client.request("textDocument/completion", self._position(text))
        items = (r or {}).get("items", []) if isinstance(r, dict) else (r or [])
        for i in items:
            if isinstance(i.get("label"), dict):
                i["label"] = i["label"].get("label", "")
        return items

    def signatures(self, text):
        r = self.client.request("textDocument/signatureHelp", self._position(text))
        return [s["label"] for s in (r or {}).get("signatures", [])]

    def close(self):
        self.client.close()


def collect(server, cloud_server, log):
    items = []

    def add(**kw):
        items.append(kw)

    deprecated = lambda i: 1 in (i.get("tags") or []) or bool(i.get("deprecated"))  # noqa: E731

    # Data types.
    types = [i["label"] for i in server.complete(probes.type_probe())]
    for t in types:
        add(area="datatypes", owner="", name=t)

    # Members per receiver: (owner, text maker for what follows the dot, static).
    receivers = []
    member = lambda decl, expr: (lambda after: probes.member_probe(decl, expr + after))  # noqa: E731
    for t in types:
        if t in probes.NOT_DECLARED:
            continue
        owner = probes.OWNER_OF_TYPE.get(t, t)
        receivers.append((owner, member(probes.DECLARATIONS.get(owner, t), "X."), False))
    for owner, decl in probes.DECLARATIONS.items():
        if not any(r[0] == owner for r in receivers):
            receivers.append((owner, member(decl, "X."), False))
    for owner, (decl_owner, expr) in probes.SUB_MEMBERS.items():
        receivers.append((owner, member(probes.DECLARATIONS[decl_owner], expr + "."), False))
    for owner, text in probes.INSIDE.items():
        receivers.append((owner, (lambda t: lambda after: t.replace("{expr}", after))(text), False))
    for name in dict.fromkeys(probes.STATIC_CANDIDATES + types):
        if name in probes.NOT_DECLARED:
            continue
        receivers.append((probes.OWNER_OF_TYPE.get(name, name), member("Integer", name + "."), True))
    receivers.append(("Enum", member("Integer", probes.ENUM_STATIC + "."), True))

    seen = set()
    for owner, make, static in receivers:
        found = [
            i for i in server.complete(make(CURSOR))
            if i.get("kind") in (KIND_METHOD, KIND_FUNCTION, KIND_PROPERTY)
            and not (i.get("detail") or "").startswith("(local)")
            and i["label"] not in probes.PROBE_MEMBERS
        ]
        if not found:
            continue
        cloud = None
        if cloud_server is not None:
            cloud = {i["label"] for i in cloud_server.complete(make(CURSOR))}
        for i in found:
            name = i["label"]
            if (owner, name, static) in seen:
                continue
            seen.add((owner, name, static))
            labels = server.signatures(make(f"{name}({CURSOR}"))
            if not labels and i.get("detail"):
                labels = [i["detail"].split(" (+")[0]]
            for label in dict.fromkeys(labels):
                n, params, ret = parse_signature(label)
                sig, named = sig_text(params)
                add(
                    area="methods", owner=owner, name=name, sig=sig, params=named, returns=ret,
                    static=static, property=i.get("kind") == KIND_PROPERTY, scope="OnPrem" if cloud is not None and name not in cloud else "",
                    deprecated=deprecated(i),
                )
        log(f"  {owner}{' (static)' if static else ''}: {len(found)} members")

    # Statement and expression keywords.
    for i in server.complete(probes.statement_probe()):
        if i.get("kind") == KIND_KEYWORD:
            add(area="syntax", owner="keyword", name=i["label"])

    # Attributes.
    for i in server.complete(probes.attribute_probe()):
        add(area="attributes", owner="", name=i["label"], deprecated=deprecated(i))

    # Trigger events.
    for obj_type, ref, owner in (("Table", 'Database::"Probe Table"', "Table"), ("Page", 'Page::"Probe Page"', "Page")):
        for i in server.complete(probes.event_probe(obj_type, ref)):
            if (i.get("detail") or "") == "Trigger Event":
                add(area="events", owner=owner, name=i["label"])

    # Triggers per owner.
    for owner, ctx in probes.TRIGGER_CONTEXTS.items():
        texts = [probes.CONTEXTS[ctx]] + probes.TRIGGER_EXTRA_CONTEXTS.get(owner, [])
        found = [i for t in texts for i in server.complete(t.replace(probes.T, "trigger " + CURSOR))]
        for i in found:
            if i.get("kind") != KIND_FUNCTION or not (i.get("detail") or "").startswith("trigger "):
                continue
            n, params, ret = parse_signature(i["detail"])
            sig, named = sig_text(params)
            add(area="triggers", owner=owner, name=i["label"], sig=sig, params=named, returns=ret,
                deprecated=deprecated(i))

    # Properties per owner, with their values where the server lists them.
    for owner, ctx in probes.CONTEXTS.items():
        props = [i for i in server.complete(ctx.replace(probes.T, CURSOR)) if i.get("kind") == KIND_PROPERTY]
        for p in props:
            name = p["label"]
            vals = server.complete(ctx.replace(probes.T, f"{name} = {CURSOR};"))
            values = [v["label"] for v in vals] if vals and all(v.get("kind") == KIND_PROPERTY for v in vals) else []
            add(area="properties", owner=owner, name=name, values=values, deprecated=deprecated(p))
        log(f"  properties {owner}: {len(props)}")
    return items


def prepare_workspace(root, major, target, system_app):
    ws = root / f"ws-{major}-{target.lower()}"
    if ws.exists():
        shutil.rmtree(ws)
    (ws / ".alpackages").mkdir(parents=True)
    if system_app:
        shutil.copy(system_app, ws / ".alpackages" / "System.app")
    (ws / "app.json").write_text(json.dumps({
        "id": "7d8a2f3e-1111-4222-8333-944455556666", "name": "Probe", "publisher": "Probe",
        "version": "1.0.0.0", "platform": f"{major}.0.0.0", "runtime": f"{major - 11}.0",
        "target": target, "idRanges": [{"from": 50100, "to": 50199}],
    }, indent=2))
    (ws / "Objects.al").write_text(probes.OBJECTS)
    return ws


def item_json(it, bc):
    f = [f'"area":{json.dumps(it["area"])}', f'"owner":{json.dumps(it.get("owner", ""))}', f'"name":{json.dumps(it["name"])}']
    if it["area"] in ("methods", "triggers"):
        f.append(f'"sig":{json.dumps(it.get("sig", "()"))}')
        f.append(f'"params":{json.dumps(it.get("params", ""))}')
        if it.get("returns"):
            f.append(f'"returns":{json.dumps(it["returns"])}')
    if it.get("static"):
        f.append('"static":true')
    if it.get("property"):
        f.append('"property":true')
    if it.get("values"):
        f.append('"values":[' + ",".join(json.dumps(v) for v in it["values"]) + "]")
    if it.get("scope"):
        f.append(f'"scope":{json.dumps(it["scope"])}')
    if it.get("deprecated"):
        f.append('"deprecated":true')
    f.append('"bc":[' + ",".join(str(b) for b in bc) + "]")
    return "{" + ",".join(f) + "}"


def main(argv):
    """argv: --out FILE --work DIR then MAJOR=HOST_CMD|SYSTEM_APP|EXTENSION_VERSION ..."""
    out, work, specs = None, None, []
    i = 0
    while i < len(argv):
        if argv[i] == "--out":
            out = argv[i + 1]
            i += 2
        elif argv[i] == "--work":
            work = pathlib.Path(argv[i + 1])
            i += 2
        else:
            major, _, rest = argv[i].partition("=")
            host, system_app, version = rest.split("|")
            specs.append((int(major), host.split("\t"), system_app, version))
            i += 1
    log = lambda m: print(m, file=sys.stderr, flush=True)  # noqa: E731
    merged, sources = {}, []
    for major, host, system_app, version in sorted(specs):
        log(f"BC {major}: AL Language {version}")
        sources.append(f'{{"bc":{major},"al_extension":{json.dumps(version)}}}')
        cloud = Server(host, prepare_workspace(work, major, "Cloud", system_app))
        onprem = Server(host, prepare_workspace(work, major, "OnPrem", system_app))
        try:
            items = collect(onprem, cloud, log)
        finally:
            onprem.close()
            cloud.close()
        for it in items:
            key = (it["area"], it.get("owner", ""), it["name"], it.get("sig", ""), bool(it.get("static")))
            if key not in merged:
                merged[key] = [it, []]
            else:
                merged[key][0] = it  # the newest major describes the item
            if major not in merged[key][1]:
                merged[key][1].append(major)
    lines = [item_json(it, bc) for _, (it, bc) in sorted(merged.items(), key=lambda kv: tuple(str(x) for x in kv[0]))]
    text = (
        "{\n"
        '"generated_by": "tools/language-catalog/generate.sh (the ignored test language_catalog_is_current checks it)",\n'
        '"source": "Answers of the AL language server of each BC major to probe files: completion lists and signature help (names and signatures only)",\n'
        f'"sources": [{", ".join(sources)}],\n'
        '"items": [\n' + ",\n".join(lines) + "\n]\n}\n"
    )
    pathlib.Path(out).write_text(text)
    log(f"{len(lines)} items written to {out}")


if __name__ == "__main__":
    main(sys.argv[1:])

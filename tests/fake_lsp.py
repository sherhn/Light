#!/usr/bin/env python3
"""A stand-in for clangd in the UI tests: a language server (stdin/stdout, JSON-RPC) that knows C the way a regular
expression does - enough to show that the IDE asks, understands the answers and shows them.

It counts characters in UTF-16 code units (it does not offer UTF-8, unlike the real clangd), so the IDE's conversion of
positions is exercised; the fixtures stay within the Basic Multilingual Plane, where that is Python's own count.

What it knows: a function is defined by `<type> name(`; every other whole-word match is a use; `missing` is an
undeclared identifier; after `.` the members are `x` and `y`; formatting squeezes spaces. With
LIGHT_FAKE_LSP_OPEN_ONLY=1 its uses and renames see only the files the IDE has open, as clangd's do in a project without
a compile_commands.json.
"""
import json
import os
import pathlib
import re
import sys
from urllib.parse import quote, unquote

DOCS = {}  # uri -> text the IDE sent
ROOT = None
LOG = os.environ.get("LIGHT_FAKE_LSP_LOG")
OPEN_ONLY = os.environ.get("LIGHT_FAKE_LSP_OPEN_ONLY") == "1"
KEYWORDS = ["int", "return", "struct", "void", "while", "if", "else", "for"]
DEF = r"\b(?:int|void|char|double|float|long|struct \w+)\s+\*?{}\s*\("


def log(*args):
    if LOG:
        with open(LOG, "a", encoding="utf-8") as f:
            print(*args, file=f)


def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.decode("ascii", "replace").strip()
        if not line:
            if length is not None:
                break
            continue
        k, _, v = line.partition(":")
        if k.lower() == "content-length":
            length = int(v)
    return json.loads(sys.stdin.buffer.read(length))


def send(msg):
    msg["jsonrpc"] = "2.0"
    body = json.dumps(msg).encode("utf-8")
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


def to_path(uri):
    p = unquote(uri[len("file://"):])
    if re.match(r"^/[A-Za-z]:", p):
        p = p[1:]
    return pathlib.Path(p)


def to_uri(path):
    s = str(path).replace("\\", "/")
    return "file:///" + quote(s.lstrip("/"), safe="/:")


def text_of(uri):
    if uri in DOCS:
        return DOCS[uri]
    try:
        return to_path(uri).read_text(encoding="utf-8").replace("\r\n", "\n")
    except OSError:
        return ""


def project():
    """uri -> text of every C file of the project (the open ones as the IDE has them)."""
    files = {}
    for p in sorted(ROOT.rglob("*")):
        if p.suffix in (".c", ".h") and ".git" not in p.parts:
            files[to_uri(p)] = None
    for uri in DOCS:
        files[uri] = None
    return {u: text_of(u) for u in files}


def offset(text, pos):
    lines = text.split("\n")
    line = min(pos["line"], len(lines) - 1)
    return sum(len(l) + 1 for l in lines[:line]) + min(pos["character"], len(lines[line]))


def position(text, off):
    before = text[:off]
    line = before.count("\n")
    return {"line": line, "character": off - (before.rfind("\n") + 1)}


def rng(text, a, b):
    return {"start": position(text, a), "end": position(text, b)}


def word_at(text, off):
    for m in re.finditer(r"\w+", text):
        if m.start() <= off <= m.end():
            return m.group(), m.start()
    return None, off


def uses(word):
    out = []
    # without a compile_commands.json clangd knows the uses in the files the IDE has open, and no more
    files = {u: t for u, t in project().items() if u in DOCS} if OPEN_ONLY else project()
    for uri, text in files.items():
        for m in re.finditer(r"\b%s\b" % re.escape(word), text):
            out.append((uri, text, m.start(), m.end()))
    return out


def definition(word):
    for uri, text in project().items():
        m = re.search(DEF.format(re.escape(word)), text)
        if m:
            a = text.index(word, m.start())
            return uri, text, a, a + len(word), text[text.rfind("\n", 0, m.start()) + 1:text.find("\n", m.end())].rstrip("{ ").strip()
    return None


def diagnostics(uri):
    text = DOCS[uri]
    found = [{"range": rng(text, m.start(), m.end()), "severity": 1, "source": "fake", "message": "use of undeclared identifier 'missing'"} for m in re.finditer(r"\bmissing\b", text)]
    return {"method": "textDocument/publishDiagnostics", "params": {"uri": uri, "version": VERSIONS.get(uri), "diagnostics": found}}


VERSIONS = {}


def answer(method, p):
    if method == "initialize":
        global ROOT
        ROOT = to_path(p["rootUri"])
        return {"capabilities": {
            "textDocumentSync": {"openClose": True, "change": 1, "save": True},
            "definitionProvider": True, "referencesProvider": True, "renameProvider": True, "hoverProvider": True,
            "documentFormattingProvider": True, "completionProvider": {"triggerCharacters": [".", "->"]},
        }, "serverInfo": {"name": "fake-clangd"}}
    if method == "shutdown":
        return None
    uri = p.get("textDocument", {}).get("uri")
    text = text_of(uri) if uri else ""
    off = offset(text, p["position"]) if "position" in p else 0
    word, start = word_at(text, off)
    if method == "textDocument/definition":
        d = word and definition(word)
        return [{"uri": d[0], "range": rng(d[1], d[2], d[3])}] if d else []
    if method == "textDocument/references":
        return [{"uri": u, "range": rng(t, a, b)} for u, t, a, b in uses(word)] if word else []
    if method == "textDocument/rename":
        changes = {}
        for u, t, a, b in uses(word or ""):
            changes.setdefault(u, []).append({"range": rng(t, a, b), "newText": p["newName"]})
        return {"changes": changes}
    if method == "textDocument/hover":
        d = word and definition(word)
        if not d:
            return None
        return {"contents": {"kind": "markdown", "value": f"### function `{word}`  \n\n---\nFrom the fake server.\n\n---\n```c\n{d[4]}\n```"}}
    if method == "textDocument/completion":
        if text[:off].endswith("."):
            return {"isIncomplete": False, "items": [{"label": "x", "kind": 5}, {"label": "y", "kind": 5}]}
        prefix = text[start:off]
        names = sorted({m.group() for t in project().values() for m in re.finditer(r"\b[A-Za-z_]\w*", t)} | set(KEYWORDS))
        items = [{"label": " " + n, "insertText": n, "filterText": n, "sortText": ("0" if definition(n) else "1") + n, "kind": 3 if definition(n) else (14 if n in KEYWORDS else 6)}
                 for n in names if n.startswith(prefix) and n != prefix]
        return {"isIncomplete": False, "items": items}
    if method == "textDocument/formatting":
        new = re.sub(r"[ \t]{2,}", " ", text)
        new = re.sub(r"\(\s+", "(", new)
        new = re.sub(r"\s+\)", ")", new)
        new = re.sub(r"\)\{", ") {", new)
        return [] if new == text else [{"range": rng(text, 0, len(text)), "newText": new}]
    raise KeyError(method)


def main():
    while True:
        msg = read()
        method = msg.get("method")
        p = msg.get("params") or {}
        log("<-", method, msg.get("id"))
        if "id" not in msg:
            if method == "exit":
                sys.exit(0)
            if method in ("textDocument/didOpen", "textDocument/didChange"):
                uri = p["textDocument"]["uri"]
                DOCS[uri] = p["textDocument"]["text"] if method.endswith("didOpen") else p["contentChanges"][-1]["text"]
                VERSIONS[uri] = p["textDocument"].get("version")
                send(diagnostics(uri))
            elif method == "textDocument/didClose":
                DOCS.pop(p["textDocument"]["uri"], None)
            continue
        try:
            send({"id": msg["id"], "result": answer(method, p)})
        except KeyError:
            send({"id": msg["id"], "error": {"code": -32601, "message": f"not supported: {method}"}})


if __name__ == "__main__":
    main()

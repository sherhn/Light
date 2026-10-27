"""Helpers for writing the modules of the registry (see tools/build_registry.py). A module is a Python dict that
becomes `module.json`; these functions build its grammar rules so the regular expressions can be written raw."""


def match(pattern, cls=None, captures=None, consume=0, ignore_case=False):
    r = {"match": pattern}
    if cls:
        r["class"] = cls
    if captures:
        r["captures"] = {str(k): v for k, v in captures.items()}
    if consume:
        r["consume"] = consume
    if ignore_case:
        r["ignore_case"] = True
    return r


def words(ws, cls, ignore_case=False):
    r = {"words": list(ws), "class": cls}
    if ignore_case:
        r["ignore_case"] = True
    return r


def region(begin, end, cls):
    return {"begin": begin, "end": end, "class": cls}


def scoped(pattern, grammar, fallback="preproc"):
    """Tokenize only inside a match; older engines colour the entire match with the fallback class."""
    return {"match": pattern, "class": fallback, "grammar": grammar}


def call(cls="function"):
    """A name followed by `(`: coloured as a function, the bracket stays for the bracket rule."""
    return match(r"\b([A-Za-z_]\w*)\(", captures={1: cls}, consume=1)


def outline(pattern, kind="function", name=1, level=0, level_mode="length"):
    o = {"pattern": pattern, "kind": kind, "name": name}
    if level:
        o["level"] = level
        o["level_mode"] = level_mode
    return o


def module(**kw):
    """Fill in what every module has."""
    m = {"version": "1.0.1", "cover": "cover.png", "license": "MIT", "author": "Light contributors"}
    m.update(kw)
    return m

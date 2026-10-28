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


# Node.js for the language servers that are npm packages: the LTS release, from nodejs.org. Each SHA-256 is the one in
# the release's SHASUMS256.txt, whose GPG signature by a Node.js releaser (keys of github.com/nodejs/release-keys) was
# checked; the archives for linux-x64 and win-x64 were downloaded and matched too.
NODE = {"name": "node", "version": "24.22.2", "source": "nodejs.org", "archives": {
    "linux-x64": {"url": "https://nodejs.org/dist/v24.22.2/node-v24.22.2-linux-x64.tar.gz", "sha256": "361718c444e49af2ca5a110c83ba6a8901143f23ab285ebbef0341f391f482a9", "size": 58299872, "strip": 1, "bin": "bin"},
    "linux-arm64": {"url": "https://nodejs.org/dist/v24.22.2/node-v24.22.2-linux-arm64.tar.gz", "sha256": "95c264150aceb7f3437a29fd6984924f5f8d1511ea3cbd6c508b83f6c555dae2", "size": 58030966, "strip": 1, "bin": "bin"},
    "darwin-x64": {"url": "https://nodejs.org/dist/v24.22.2/node-v24.22.2-darwin-x64.tar.gz", "sha256": "d5e336d588c45be0c37dbd52cc29895593009ca4997a77a1fd02815131ed0e52", "size": 54383415, "strip": 1, "bin": "bin"},
    "darwin-arm64": {"url": "https://nodejs.org/dist/v24.22.2/node-v24.22.2-darwin-arm64.tar.gz", "sha256": "aa9549349e47dffaa42dcf95d6e06ade8c4bdafc980979883b88767670df385a", "size": 53068959, "strip": 1, "bin": "bin"},
    "win-x64": {"url": "https://nodejs.org/dist/v24.22.2/node-v24.22.2-win-x64.zip", "sha256": "ca1bff5d8ead13c9f2ea404a176db46bd430704d5b5113734049fa365e7799e2", "size": 37754834, "strip": 1, "bin": ""},
    "win-arm64": {"url": "https://nodejs.org/dist/v24.22.2/node-v24.22.2-win-arm64.zip", "sha256": "f47968c213105077f64c6510f007632c0ca2574802a0b5067c327c49abe1c468", "size": 33789289, "strip": 1, "bin": ""},
}}


def npm_server(name, languages, command, version, size, **kw):
    """A language server Light installs from npm, on NODE. Its package.json and package-lock.json are in
    tools/registry_src/lsp/<name>/ (build_registry.py puts them into the module as <name>.package.json and
    <name>.package-lock.json); every package of the lockfile was reviewed (licence, no install scripts run)."""
    return dict(languages=languages, name=name, command=command, program=command.split()[0],
                install=dict(version=version, source="registry.npmjs.org", size=size, runtime=NODE, npm=name), **kw)

#!/usr/bin/env python3
"""Build the module registry: registry/modules/<id>/module.json (from tools/registry_src), the covers and
registry/index.json with the size and SHA-256 of every file.

Usage:
  python tools/build_registry.py            # modules + index (covers that exist are kept)
  python tools/build_registry.py --covers   # draw the covers again as well

The IDE downloads registry/ from raw.githubusercontent.com (or reads a copy shipped next to it) and checks every
file against index.json, so run this after changing a module and commit the result.
"""
import hashlib
import importlib
import json
import pathlib
import shutil
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "tools" / "registry_src"
OUT = ROOT / "registry"
ORDER = ["lisp", "c_cpp", "python", "assembler", "html", "css", "markdown", "restructuredtext", "docker", "javascript", "shell_make"]


def load_modules():
    sys.path.insert(0, str(SRC))
    return [importlib.import_module(n).MODULE for n in ORDER]


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")


def module_notices(module, destination):
    """Legal materials travel with every independently downloadable module."""
    shutil.copyfile(ROOT / "LICENSE", destination / "LICENSE")
    lines = ["# Module notices", "", "Light's original grammar, templates and artwork are MIT licensed; see LICENSE.", ""]
    mark = {"html": "HTML5", "css": "CSS", "markdown": "Markdown"}.get(module["id"])
    if mark:
        shutil.copyfile(ROOT / "assets/module-marks/SimpleIcons-LICENSE.md", destination / "SimpleIcons-CC0.txt")
        lines += ["The SVG conversion comes from Simple Icons (CC0); original artwork retains its own conditions.", "Source: https://github.com/simple-icons/simple-icons", ""]
    if mark == "HTML5":
        shutil.copyfile(ROOT / "assets/module-marks/HTML5-CC-BY-3.0.txt", destination / "HTML5-CC-BY-3.0.txt")
        lines += ["HTML5 Logo by W3C: https://www.w3.org/html/logo/", "CC BY 3.0: https://creativecommons.org/licenses/by/3.0/", "The cover changes the logo's color, opacity and background composition."]
    elif mark == "CSS":
        lines += ["CSS logo: https://github.com/CSS-Next/logo.css (CC0-1.0)."]
    elif mark == "Markdown":
        lines += ["Markdown Mark by Dustin Curtis: https://github.com/dcurtis/markdown-mark (CC0-1.0)."]
    else:
        lines += ["The cover uses Light's own typographic label."]
    if module["id"] == "python":
        lines += ["Python is a trademark of the Python Software Foundation; the name identifies the supported programming language.", "Light is not affiliated with the PSF. No Python logo is reproduced in this cover."]
    (destination / "THIRD_PARTY_NOTICES.md").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def main(argv):
    redraw = "--covers" in argv
    mods = load_modules()
    entries = []
    for m in mods:
        d = OUT / "modules" / m["id"]
        d.mkdir(parents=True, exist_ok=True)
        write_json(d / "module.json", m)
        module_notices(m, d)
        # the lockfiles of the language servers Light installs with npm: <name>.package.json, <name>.package-lock.json
        for server in m.get("lsp", []):
            npm = (server.get("install") or {}).get("npm")
            if npm:
                for f in ("package.json", "package-lock.json"):
                    shutil.copyfile(SRC / "lsp" / npm / f, d / f"{npm}.{f}")
        cover = d / m["cover"]
        if redraw or not cover.exists():
            sys.path.insert(0, str(ROOT / "tools"))
            import covers
            covers.draw(m, cover)
        files = []
        for f in sorted(d.iterdir(), key=lambda p: (p.name != "module.json", p.name)):
            if f.is_file():
                data = f.read_bytes()
                files.append({"name": f.name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()})
        entries.append({
            "id": m["id"], "name": m["name"], "version": m["version"], "category": m["category"], "summary": m["summary"],
            "cover": m["cover"], "path": f"modules/{m['id']}", "files": files, "detect": m.get("detect", {}),
        })
    write_json(OUT / "index.json", {"version": 1, "modules": entries})
    print(f"registry: {len(entries)} modules, {sum(f['size'] for e in entries for f in e['files']) // 1024} KB in {OUT}")


if __name__ == "__main__":
    main(sys.argv[1:])

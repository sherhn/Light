"""Shared file-icon catalog, derived from the pinned language modules."""
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "assets/file-types.json"


def generate():
    index = json.loads((ROOT / "registry/index.json").read_text(encoding="utf-8"))
    types = []
    for entry in index["modules"]:
        module = json.loads((ROOT / "registry" / entry["path"] / "module.json").read_text(encoding="utf-8"))
        extensions, names = set(), set()
        for language in module.get("languages", []):
            extensions.update(e.lower() for e in language.get("extensions", []))
            names.update(language.get("filenames", []))
        # Lisp lives in the core engine. Detection also covers tool/configuration files.
        extensions.update(e.lower() for e in entry["detect"]["extensions"])
        types.append({"id": entry["id"], "name": entry["name"], "extensions": sorted(extensions), "files": sorted(names)})
    # Add module detection names only when no language already claims the name.
    claimed = {name.lower() for kind in types for name in kind["files"]}
    for entry, kind in zip(index["modules"], types):
        for name in entry["detect"]["files"]:
            if name.lower() not in claimed:
                kind["files"].append(name)
                claimed.add(name.lower())
        kind["files"].sort()
    claimed_extensions = set()
    for kind in types:
        kind["extensions"] = [extension for extension in kind["extensions"] if extension not in claimed_extensions]
        claimed_extensions.update(kind["extensions"])
    # SQL is built into the IDE (its module is seeded from this repository, not the pinned catalog).
    types.append({"id": "sql", "name": "SQL", "extensions": ["sql"], "files": []})
    # Keep existing Open With text formats and Light run scenarios available.
    types.append({"id": "source", "name": "Source code", "extensions": ["als", "rs", "ts", "tsx", "txt"], "files": []})
    return types


def load():
    return json.loads(CATALOG.read_text(encoding="utf-8"))


def extensions():
    return {extension: kind for kind in load() for extension in kind["extensions"]}


def nsis_macros(prefix="Software"):
    lines = ["; Generated from assets/file-types.json. Never writes UserChoice.", "!macro RegisterFileTypes"]
    for kind in load():
        progid = "Light.Source." + kind["id"]
        lines += [f'  WriteRegStr HKCU "Software\\Classes\\{progid}" "" "{kind["name"]} (Light)"',
                  f'  WriteRegStr HKCU "Software\\Classes\\{progid}\\DefaultIcon" "" \'$\\"$INSTDIR\\file-icons\\{kind["id"]}.ico$\\",0\'',
                  f'  WriteRegStr HKCU "Software\\Classes\\{progid}\\shell\\open\\command" "" \'$\\"$INSTDIR\\light.exe$\\" --view $\\"%1$\\"\'']
    for extension, kind in sorted(extensions().items()):
        progid = "Light.Source." + kind["id"]
        lines += [f'  WriteRegStr HKCU "Software\\Classes\\Applications\\light.exe\\SupportedTypes" ".{extension}" ""',
                  f'  WriteRegStr HKCU "Software\\Classes\\.{extension}\\OpenWithProgids" "{progid}" ""',
                  f'  WriteRegStr HKCU "Software\\Light\\Capabilities\\FileAssociations" ".{extension}" "{progid}"']
    lines += ["!macroend", "!macro UnregisterFileTypes"]
    for extension, kind in sorted(extensions().items()):
        lines.append(f'  DeleteRegValue HKCU "Software\\Classes\\.{extension}\\OpenWithProgids" "Light.Source.{kind["id"]}"')
    for kind in load():
        lines.append(f'  DeleteRegKey HKCU "Software\\Classes\\Light.Source.{kind["id"]}"')
    return "\n".join(lines + ["!macroend", ""]).replace("Software\\", prefix + "\\")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    generated = generate()
    if args.check:
        assert load() == generated, "Run python tools/file_types.py after changing the pinned modules"
        print("File icon catalog covers the pinned module languages")
    else:
        CATALOG.write_text(json.dumps(generated, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"Wrote {len(generated)} file icon families")

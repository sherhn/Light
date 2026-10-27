#!/usr/bin/env python3
"""Collect exact upstream notices; reject incomplete or stale redistribution bundles.

Run ``refresh`` after changing Cargo.lock, then review and commit assets/licenses/rust.
Package builds consume that reviewed catalogue without fetching floating license texts.
"""
import argparse
import concurrent.futures
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess
import tarfile
import threading
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
CATALOGUE = ROOT / "assets/licenses/rust"
CACHE = ROOT / "target/legal-cache"
ALLOWED = {"0BSD", "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "ISC", "Zlib", "BSD-2-Clause", "BSD-3-Clause", "BSL-1.0", "Unicode-3.0", "MPL-2.0", "OFL-1.1", "Ubuntu-font-1.0", "Unlicense", "NCSA", "CC0-1.0"}
_locks = {}
_locks_guard = threading.Lock()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def json_file(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")


def relative_path(value):
    relative = PurePosixPath(value)
    if relative.is_absolute() or ".." in relative.parts or "\\" in value or ":" in value:
        raise ValueError(f"unsafe legal-material path: {value}")
    return relative


def _download(url, destination, expected=None, maximum=150_000_000):
    """Download data only, atomically; never execute retrieved source or build recipes."""
    destination = Path(destination)
    if destination.is_file() and (expected is None or digest(destination.read_bytes()) == expected):
        return destination
    if not url.startswith("https://"):
        raise ValueError(f"source URL must use HTTPS: {url}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(destination.suffix + ".download")
    try:
        request = urllib.request.Request(url, headers={"User-Agent": "Light-License-Bundle/1.0"})
        with urllib.request.urlopen(request, timeout=45) as response, temporary.open("wb") as stream:
            total = 0
            while block := response.read(1024 * 1024):
                total += len(block)
                if total > maximum:
                    raise ValueError(f"source exceeds {maximum} bytes: {url}")
                stream.write(block)
        if expected and digest(temporary.read_bytes()) != expected:
            raise ValueError(f"SHA-256 mismatch: {url}")
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)
    return destination


def download(url, destination, expected=None, maximum=150_000_000):
    with _locks_guard:
        mutex = _locks.setdefault(str(Path(destination).resolve()), threading.Lock())
    with mutex:
        return _download(url, destination, expected, maximum)


def metadata():
    result = subprocess.run(["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT, check=True, capture_output=True, text=True)
    meta = json.loads(result.stdout)
    return sorted((p for p in meta["packages"] if p["id"] not in meta["workspace_members"]), key=lambda p: (p["name"], p["version"]))


def lock_packages():
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    return {(p["name"], p["version"]): p for p in lock["package"] if p.get("source")}


def choose_license(expression):
    """Resolve AND/OR including parentheses; never treat an AND as an alternative."""
    expression = expression.replace("/", " OR ")
    tokens = re.findall(r"\(|\)|[^\s()]+", expression)
    cursor = 0

    def atom():
        nonlocal cursor
        if cursor >= len(tokens):
            raise ValueError(f"incomplete license expression: {expression}")
        if tokens[cursor] == "(":
            cursor += 1
            result = alternatives()
            if cursor >= len(tokens) or tokens[cursor] != ")":
                raise ValueError(f"unbalanced license expression: {expression}")
            cursor += 1
            return result
        identifier = tokens[cursor]
        cursor += 1
        if cursor < len(tokens) and tokens[cursor] == "WITH":
            identifier += " WITH " + tokens[cursor + 1]
            cursor += 2
        return [{identifier}]

    def conjunction():
        nonlocal cursor
        result = atom()
        while cursor < len(tokens) and tokens[cursor] == "AND":
            cursor += 1
            right = atom()
            result = [a | b for a in result for b in right]
        return result

    def alternatives():
        nonlocal cursor
        result = conjunction()
        while cursor < len(tokens) and tokens[cursor] == "OR":
            cursor += 1
            result += conjunction()
        return result

    candidates = alternatives()
    if cursor != len(tokens):
        raise ValueError(f"invalid license expression: {expression}")
    candidates = [c for c in candidates if c <= ALLOWED]
    if not candidates:
        raise ValueError(f"license requires review: {expression}")
    preference = {name: i for i, name in enumerate(["0BSD", "MIT", "ISC", "Zlib", "BSD-2-Clause", "BSD-3-Clause", "Apache-2.0", "Unlicense"])}
    return sorted(min(candidates, key=lambda c: (len(c), sum(preference.get(x, 20) for x in c), sorted(c))))


def covered(identifier, texts):
    text = " ".join("\n".join(texts).lower().split())
    signatures = {
        "MIT": ["permission is hereby granted, free of charge"],
        "Apache-2.0": ["apache license", "version 2.0"],
        "Apache-2.0 WITH LLVM-exception": ["apache license", "llvm exceptions"],
        "ISC": ["permission to use, copy, modify", "permission notice"],
        "Zlib": ["origin of this software must not be misrepresented"],
        "BSD-2-Clause": ["redistributions in binary form", "redistributions of source code"],
        "BSD-3-Clause": ["redistributions in binary form", "neither the name"],
        "0BSD": ["permission to use, copy, modify", "for any purpose"],
        "BSL-1.0": ["boost software license"],
        "Unicode-3.0": ["unicode", "permission"],
        "MPL-2.0": ["mozilla public license", "2.0"],
        "OFL-1.1": ["sil open font license", "1.1"],
        "Ubuntu-font-1.0": ["ubuntu font licence", "1.0"],
        "Unlicense": ["this is free and unencumbered software"],
        "NCSA": ["university of illinois", "redistributions"],
        "CC0-1.0": ["cc0", "public domain"],
    }
    return all(s in text for s in signatures[identifier])


def font_copyright(data):
    tables = {tag: (offset, length) for tag, _, offset, length in (struct.unpack_from(">4sIII", data, 12 + i * 16) for i in range(struct.unpack_from(">H", data, 4)[0]))}
    base, _ = tables[b"name"]
    _, count, strings = struct.unpack_from(">HHH", data, base)
    notices = set()
    for i in range(count):
        platform, _, _, name, size, offset = struct.unpack_from(">6H", data, base + 6 + i * 12)
        if name in (0, 13, 14):
            raw = data[base + strings + offset:base + strings + offset + size]
            notices.add(raw.decode("utf-16-be" if platform in (0, 3) else "mac_roman", errors="strict"))
    return "\n\n".join(sorted(notices)) + "\n"


def upstream_licenses(package):
    base = Path(package["manifest_path"]).parent
    vcs = json.loads((base / ".cargo_vcs_info.json").read_text(encoding="utf-8"))
    match = re.match(r"https?://github.com/([^/]+/[^/#]+)", package.get("repository") or "")
    if not match:
        raise ValueError(f"cannot locate exact upstream license for {package['name']}")
    repository = match[1].removesuffix(".git")
    revision = vcs["git"]["sha1"]
    # Some 0.37.0 crates refer to an unpublished preparation commit. This is
    # the independently resolved commit of upstream's annotated 0.37.0 release tag;
    # preserve the license of that exact release rather than using main/HEAD.
    if repository == "emilk/egui" and package["version"] == "0.37.0":
        revision = "841a328ba2dd8f6996beb7189ebb7fc00e668a2a"
    key = f"{repository.replace('/', '-')}-{revision}"
    with _locks_guard:
        mutex = _locks.setdefault(key, threading.Lock())
    with mutex:
        archive = download(f"https://codeload.github.com/{repository}/tar.gz/{revision}", CACHE / (key + ".tar.gz"))
    ancestors = [PurePosixPath(vcs.get("path_in_vcs", "")), *PurePosixPath(vcs.get("path_in_vcs", "")).parents]
    result = {}
    with tarfile.open(archive, "r:gz") as source:
        for member in source.getmembers():
            relative = PurePosixPath(*PurePosixPath(member.name).parts[1:])
            is_notice = relative.parent in ancestors and re.match(r"(?:licen[cs]e|copying|notice|copyright|authors)(?:$|[-_.])", relative.name, re.I)
            in_license_dir = any(part.lower() in ("license", "licenses") for part in relative.parts[:-1])
            if (member.isfile() or member.issym() or member.islnk()) and (is_notice or in_license_dir):
                content = source.extractfile(member).read()
                content.decode("utf-8")
                result[relative.as_posix().replace("/", "__")] = (content, f"https://github.com/{repository}/blob/{revision}/{relative}")
    return result


def collect_package(package):
    name, version = package["name"], package["version"]
    selected = choose_license(package.get("license") or "")
    base = Path(package["manifest_path"]).parent
    origin = f"https://crates.io/crates/{name}/{version}"
    files = {}
    for path in sorted(base.rglob("*")):
        if path.is_file() and re.search(r"licen[cs]e|copying|notice|copyright|authors", path.name, re.I):
            data = path.read_bytes()
            try:
                data.decode("utf-8")
            except UnicodeDecodeError:
                continue
            files[path.relative_to(base).as_posix()] = (data, origin)
    if name == "epaint_default_fonts":
        for path in sorted((base / "fonts").glob("*.txt")):
            files["fonts/" + path.name] = (path.read_bytes(), origin)
        copyrights = "\n".join(path.name + "\n" + font_copyright(path.read_bytes()) for path in sorted((base / "fonts").glob("*.ttf")))
        files["fonts/FONT-COPYRIGHTS.txt"] = (copyrights.encode("utf-8"), origin)
    texts = lambda: [data.decode("utf-8") for data, _ in files.values()]
    if not all(covered(license_id, texts()) for license_id in selected):
        for filename, data in upstream_licenses(package).items():
            files["upstream/" + filename] = data
    # objc2 deliberately publishes its MIT grant as a declaration with a link
    # instead of reproducing the standard text. Retain that original declaration
    # and its package author credits, and supply the standard MIT terms alongside.
    objc_declaration = "github.com/madsmtm/objc2" in (package.get("repository") or "")
    dispatch_declaration = name == "dispatch" and version == "0.2.0" and "github.com/SSheldon/rust-dispatch" in (package.get("repository") or "")
    if selected == ["MIT"] and (objc_declaration or dispatch_declaration):
        declaration = "\n".join(texts())
        if dispatch_declaration:
            files["upstream/Cargo.toml.orig"] = ((base / "Cargo.toml.orig").read_bytes(), "https://github.com/SSheldon/rust-dispatch/blob/82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748/Cargo.toml")
        if dispatch_declaration or "https://opensource.org/license/MIT" in declaration:
            standard = "Permission is hereby granted" + (ROOT / "LICENSE").read_text(encoding="utf-8").split("Permission is hereby granted", 1)[1]
            credits = "MIT License\n\nUpstream package authors: " + "; ".join(package.get("authors", [])) + "\nUpstream MIT declaration and original notices are preserved in this directory.\n\n"
            files["MIT-standard-text.txt"] = ((credits + standard).encode("utf-8"), "https://opensource.org/license/MIT")
    missing = [license_id for license_id in selected if not covered(license_id, texts())]
    if missing:
        raise ValueError(f"{name} {version}: missing full license text for {missing}")
    destination = CATALOGUE / f"{name}-{version}"
    rows = []
    for relative, (data, source) in sorted(files.items()):
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        rows.append({"path": target.relative_to(CATALOGUE).as_posix(), "sha256": digest(data), "source": source})
    print(f"Notices: {name} {version} ({', '.join(selected)})", flush=True)
    return {"name": name, "version": version, "expression": package["license"], "selected": selected, "repository": package.get("repository"), "files": rows}


def refresh():
    packages = metadata()
    rows, errors = [], []
    with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
        futures = {pool.submit(collect_package, package): package for package in packages}
        for future in concurrent.futures.as_completed(futures):
            package = futures[future]
            try:
                rows.append(future.result())
            except Exception as error:
                errors.append(f"{package['name']} {package['version']}: {error}")
    if errors:
        raise ValueError("Incomplete license collection:\n" + "\n".join(sorted(errors)))
    rows.sort(key=lambda p: (p["name"], p["version"]))
    locked = lock_packages()
    for row in rows:
        row["archive_sha256"] = locked[(row["name"], row["version"])]["checksum"]
    json_file(CATALOGUE / "index.json", {"version": 1, "packages": rows})
    validate_catalogue(packages)


def validate_catalogue(packages=None):
    packages = packages if packages is not None else metadata()
    catalogue = json.loads((CATALOGUE / "index.json").read_text(encoding="utf-8"))
    expected = {(p["name"], p["version"], p["license"]) for p in packages}
    actual = {(p["name"], p["version"], p["expression"]) for p in catalogue["packages"]}
    if actual != expected:
        raise ValueError(f"stale license catalogue; run python tools/license_bundle.py refresh (missing: {expected - actual}; obsolete: {actual - expected})")
    for package in catalogue["packages"]:
        texts = []
        for record in package["files"]:
            relative = relative_path(record["path"])
            data = (CATALOGUE / relative).read_bytes()
            if digest(data) != record["sha256"]:
                raise ValueError(f"license checksum mismatch: {relative}")
            texts.append(data.decode("utf-8"))
        if package["selected"] != choose_license(package["expression"]):
            raise ValueError(f"license selection changed: {package['name']}")
        if not all(covered(identifier, texts) for identifier in package["selected"]):
            raise ValueError(f"incomplete license texts: {package['name']}")
    return catalogue


def crate_source(package, destination):
    locked = lock_packages()[(package["name"], package["version"])]
    if not locked["source"].startswith("registry+"):
        raise ValueError("source collection requires review for non-registry dependencies")
    name = f"{package['name']}-{package['version']}.crate"
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    cached = next((p for p in (cargo_home / "registry/cache").glob("*/" + name) if digest(p.read_bytes()) == locked["checksum"]), None)
    if cached is None:
        cached = download(f"https://static.crates.io/crates/{package['name']}/{name}", CACHE / name, locked["checksum"])
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(cached, destination)
    return {"path": destination.name, "sha256": locked["checksum"], "source": f"https://crates.io/crates/{package['name']}/{package['version']}"}


def bundle_cargo(destination):
    catalogue = validate_catalogue()
    rows = ["## Rust dependencies", "", "The selected license branch is recorded below. Full original notices accompany each package.", "", "| Package | Version | Selected license | Notices |", "|---|---|---|---|"]
    source_notes = []
    for package in catalogue["packages"]:
        folder = f"{package['name']}-{package['version']}"
        for record in package["files"]:
            target = destination / "licenses/rust" / record["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(CATALOGUE / record["path"], target)
        rows.append(f"| {package['name']} | {package['version']} | {' AND '.join(package['selected'])} | licenses/rust/{folder}/ |")
        if "MPL-2.0" in package["selected"]:
            source = destination / "sources" / f"{folder}.crate"
            crate_source(package, source)
            source_notes += ["", f"The complete, unmodified MPL-2.0 source for {folder} is included in `sources/{source.name}`.", "A .crate file is a gzip-compressed tar archive; unpack it with a standard archive tool.", "MPL rights apply to that component's source independently of Light's MIT license.", ""]
    json_file(destination / "licenses/rust/index.json", catalogue)
    return "\n".join(rows + ["", "## MPL source access"] + source_notes) + "\n"


def bundle_static(destination):
    source = ROOT / "assets"
    for path in (source / "licenses").iterdir():
        if path.is_file():
            shutil.copyfile(path, destination / "licenses" / path.name)
    for path in (source / "fonts").glob("*.txt"):
        shutil.copyfile(path, destination / "licenses" / path.name)
    shutil.copyfile(source / "icons/LICENSE", destination / "licenses/Lucide-Feather-LICENSE.txt")
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    copyright_file = sysroot / "share/doc/rust/COPYRIGHT-library.html"
    if not copyright_file.is_file():
        copyright_file = sysroot / "share/doc/rust/COPYRIGHT.html"
    if not copyright_file.is_file():
        raise ValueError("Rust standard-library copyright material missing from the active toolchain")
    shutil.copyfile(copyright_file, destination / "licenses/Rust-standard-library-COPYRIGHT.html")
    compiler = subprocess.check_output(["rustc", "-Vv"], text=True)
    (destination / "licenses/Rust-toolchain.txt").write_text(compiler, encoding="utf-8")
    return "\n".join(["## Artwork, fonts and standard library", "", "* Lucide Icons and Contributors: ISC; Feather-derived icons: MIT, Cole Bemis. See licenses/Lucide-Feather-LICENSE.txt.", "* JetBrains Mono: SIL OFL 1.1, licenses/JetBrainsMono-OFL.txt and JetBrainsMono-AUTHORS.txt.", "* Inter: SIL OFL 1.1, licenses/Inter-OFL.txt.", "* egui fallback fonts (Hack, Ubuntu and egui-icons): see licenses/rust/epaint_default_fonts-*/fonts/.", "* Module artwork has its own THIRD_PARTY_NOTICES.md inside each registry/modules/<id>/ directory.", "* Rust standard library and its third-party components: licenses/Rust-standard-library-COPYRIGHT.html; exact compiler: licenses/Rust-toolchain.txt.", ""]) + "\n"


def seal(destination):
    paths = sorted(p for directory in ("licenses", "sources", "registry") for p in (destination / directory).rglob("*") if p.is_file())
    paths += [destination / name for name in ("LICENSE", "THIRD_PARTY_NOTICES.md")]
    json_file(destination / "LEGAL_MANIFEST.json", {"version": 1, "files": [{"path": p.relative_to(destination).as_posix(), "sha256": digest(p.read_bytes())} for p in paths]})


def verify(destination):
    manifest = json.loads((destination / "LEGAL_MANIFEST.json").read_text(encoding="utf-8"))
    for record in manifest["files"]:
        relative = relative_path(record["path"])
        if digest((destination / relative).read_bytes()) != record["sha256"]:
            raise ValueError(f"invalid or missing legal material: {record['path']}")
    required = ["LICENSE", "THIRD_PARTY_NOTICES.md", "licenses/Lucide-Feather-LICENSE.txt", "licenses/Inter-OFL.txt", "licenses/JetBrainsMono-OFL.txt", "licenses/rust/index.json", "licenses/SBCL-NOTICES.txt", "licenses/Rust-standard-library-COPYRIGHT.html"]
    for name in required:
        if not (destination / name).is_file():
            raise ValueError(f"missing legal material: {name}")
    index = json.loads((destination / "licenses/rust/index.json").read_text(encoding="utf-8"))
    if digest((destination / "licenses/rust/index.json").read_bytes()) != digest((CATALOGUE / "index.json").read_bytes()):
        raise ValueError("distribution uses a different Rust license catalogue")
    sealed = {record["path"] for record in manifest["files"]}
    for package in index["packages"]:
        texts = []
        for notice in package["files"]:
            relative = "licenses/rust/" + notice["path"]
            if relative not in sealed or digest((destination / relative).read_bytes()) != notice["sha256"]:
                raise ValueError(f"crate notice missing or changed: {relative}")
            texts.append((destination / relative).read_text(encoding="utf-8"))
        if not all(covered(identifier, texts) for identifier in package["selected"]):
            raise ValueError(f"crate full license missing: {package['name']}")
        if "MPL-2.0" in package["selected"]:
            source = destination / f"sources/{package['name']}-{package['version']}.crate"
            if not source.is_file() or digest(source.read_bytes()) != package["archive_sha256"]:
                raise ValueError("MPL source archive missing or changed")
    from legal_sources import SBCL_SOURCE_SHA256, SBCL_VERSION
    if digest((destination / f"sources/sbcl-{SBCL_VERSION}.tar.gz").read_bytes()) != SBCL_SOURCE_SHA256:
        raise ValueError("SBCL source archive missing or changed")
    if (destination / "git/cmd/git.exe").is_file():
        source_record = json.loads((destination / "licenses/Git-SOURCE-MANIFEST.json").read_text(encoding="utf-8"))
        from git_sources import LOCK, tree_fingerprint, versions
        locked_sources = json.loads(LOCK.read_text(encoding="utf-8"))
        if source_record["packages"] != len(versions(destination / "git")) or not source_record["sha256"] or source_record["packages"] != len(locked_sources["packages"]):
            raise ValueError("Portable Git corresponding-source manifest is incomplete")
        if tree_fingerprint(destination / "git") != locked_sources["tree_sha256"]:
            raise ValueError("Portable Git binary tree no longer matches its source kit")
    for module in (destination / "registry/modules").iterdir():
        if module.is_dir() and not all((module / name).is_file() for name in ("LICENSE", "THIRD_PARTY_NOTICES.md")):
            raise ValueError(f"module notices missing: {module.name}")
        if module.name == "html" and ("HTML5 Logo by W3C" not in (module / "THIRD_PARTY_NOTICES.md").read_text(encoding="utf-8") or not (module / "HTML5-CC-BY-3.0.txt").is_file()):
            raise ValueError("HTML5 attribution or license missing")
    frameworks = destination.parent / "Frameworks"
    if frameworks.is_dir() and any(frameworks.glob("*.dylib")):
        if "Zstandard 1.5.7" not in (destination / "THIRD_PARTY_NOTICES.md").read_text(encoding="utf-8") or not (destination / "licenses/Zstandard-1.5.7-BSD.txt").is_file():
            raise ValueError("native Zstandard notice missing")
    print(f"Legal materials verified: {len(manifest['files'])} files", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["refresh", "check", "verify"])
    parser.add_argument("path", nargs="?", type=Path)
    args = parser.parse_args()
    if args.command == "refresh":
        refresh()
    elif args.command == "check":
        validate_catalogue()
        print("License catalogue matches Cargo.lock")
    elif args.path:
        verify(args.path)
    else:
        parser.error("verify requires a distribution directory")

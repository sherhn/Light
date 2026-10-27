"""Mirror the exact source packages of the Portable Git distribution.

Upstream recipes are treated as data. This tool never executes PKGBUILD files.
Refresh creates a reviewed, checksum-pinned lock; normal builds may only consume it.
"""
import argparse
import concurrent.futures
import gzip
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile
import urllib.error
import urllib.parse
import zipfile

from license_bundle import CACHE, ROOT, digest, download, json_file

LOCK = ROOT / "tools/git-sources.lock.json"


def reconstruct_xpdf(distribution=None):
    """One upstream UCRT build omitted --allsource; recover it without running its recipe."""
    base = "mingw-w64-xpdf-tools"
    revision = "762d8468f5384d8b7015a12f66eb3ff4197fa05c"
    recipe_base = f"https://raw.githubusercontent.com/git-for-windows/MINGW-packages/{revision}/mingw-w64-xpdf/"
    binary_url = "https://github.com/git-for-windows/pacman-repo/releases/download/2026-06-16T08-29-17.595438100Z/mingw-w64-ucrt-x86_64-xpdf-tools-4.06-1-any.pkg.tar.xz"
    binary = download(binary_url, CACHE / binary_url.rsplit("/", 1)[1], "071046b73356b0f0868c5d9d5313e28b7a144b0346b54217f19e212feab9b1fb")
    build_info = archive_read(binary, ".BUILDINFO")
    expected_recipe = re.search(rb"^pkgbuild_sha256sum = ([0-9a-f]{64})$", build_info, re.M)[1].decode()
    if expected_recipe != "59f2849787f57aa20bbb1d787991c503612ee750f53116870695caf0a1710fbd":
        raise ValueError("Xpdf binary recipe changed")
    files = {"BINARY_BUILDINFO": build_info}
    inputs = [
        ("PKGBUILD", recipe_base + "PKGBUILD", expected_recipe),
        ("xpdf-4.06.tar.gz", "https://dl.xpdfreader.com/xpdf-4.06.tar.gz", "1c38f527c46caee0f712386d42a885b96a31ed9ce11904e872559859894d137e"),
        ("002-xpdf-4.00-64bit.patch", recipe_base + "002-xpdf-4.00-64bit.patch", "6a9999f85bfe40feca8fc56db13149b14024ea5dd6b33262cc723efc072700db"),
        ("005-xpdf-4.00-permissions.patch", recipe_base + "005-xpdf-4.00-permissions.patch", "85d7c4bb5798d09f42f88cb09fe2c7762daf6395e30804132aa39bcf28b40b31"),
        ("006-xpdf-4.00-only-tools.patch", recipe_base + "006-xpdf-4.00-only-tools.patch", "636b9caa729b6fe686e8b3dd1c1bc91d299687bbd81d0dcd5c4018b7fe255fe8"),
    ]
    for name, url, expected in inputs:
        files[name] = download(url, CACHE / "xpdf-inputs" / name, expected).read_bytes()
    files["xpdf-4.06.tar.gz.sig"] = download("https://dl.xpdfreader.com/xpdf-4.06.tar.gz.sig", CACHE / "xpdf-inputs/xpdf-4.06.tar.gz.sig").read_bytes()
    if distribution:
        original_binary = archive_read(binary, "ucrt64/bin/pdftotext.exe")
        if original_binary != (distribution / "ucrt64/bin/pdftotext.exe").read_bytes():
            raise ValueError("Xpdf receipt does not correspond to bundled pdftotext.exe")
    files[".SRCINFO"] = (f"pkgbase = {base}\n\tpkgver = 4.06\n\tpkgrel = 1\n\tlicense = GPL-2.0-only OR GPL-3.0-only\n" + "".join(f"\tsource = {name}\n" for name, _, _ in inputs if name != "PKGBUILD") + "\tsource = xpdf-4.06.tar.gz.sig\n").encode()
    files["PROVENANCE.json"] = (json.dumps({"reconstructed": True, "explanation": "Complete source and patches reconstructed from PKGBUILD with SHA-256 matching the distributed binary's BUILDINFO; .SRCINFO is generated for this source kit.", "recipe_revision": revision, "binary_url": binary_url, "inputs": [{"path": name, "url": url, "sha256": expected} for name, url, expected in inputs]}, indent=2) + "\n").encode()
    target = CACHE / "git-source-packages" / f"{base}-4.06-1.src.tar.gz"
    target.parent.mkdir(parents=True, exist_ok=True)
    # Fixed gzip/tar metadata makes the reconstructed, reviewed archive reproducible.
    with target.open("wb") as stream, gzip.GzipFile(filename="", fileobj=stream, mode="wb", mtime=0) as compressed, tarfile.open(fileobj=compressed, mode="w") as archive:
        for name, data in sorted(files.items()):
            info = tarfile.TarInfo(base + "/" + name)
            info.size, info.mtime, info.mode = len(data), 0, 0o644
            archive.addfile(info, io.BytesIO(data))
    inspect_source(target, base, "4.06-1")
    return {"name": "mingw-w64-ucrt-x86_64-xpdf-tools", "version": "4.06-1", "base": base, "archive": target.name, "sha256": digest(target.read_bytes()), "url": recipe_base + "PKGBUILD", "reconstructed": True, "licenses": ["GPL-2.0-only OR GPL-3.0-only"]}


def archive_names(path):
    return subprocess.check_output(["tar", "-tf", str(path)], text=True, encoding="utf-8").splitlines()


def archive_read(path, name):
    return subprocess.check_output(["tar", "-xOf", str(path), name])


def database(path):
    try:
        with tarfile.open(path) as archive:
            contents = b"\n".join(archive.extractfile(m).read() for m in archive.getmembers() if m.isfile() and m.name.endswith("/desc"))
    except tarfile.ReadError:
        bsd = "bsdtar" in subprocess.check_output(["tar", "--version"], text=True)
        select = ["--include=*/desc"] if bsd else ["--wildcards", "*/desc"]
        contents = subprocess.check_output(["tar", "-xOf", str(path), *select])
    packages = {}
    for block in re.split(r"(?=%FILENAME%\n)", contents.decode("utf-8").replace("\r\n", "\n")):
        fields = {key: value.strip().splitlines() for key, value in re.findall(r"%([^%\n]+)%\n(.*?)(?=\n\n|$)", block, re.S)}
        if "NAME" in fields:
            packages[fields["NAME"][0]] = fields
    return packages


def versions(distribution):
    data = (distribution / "etc/package-versions.txt").read_bytes()
    rows = [line.split() for line in data.decode("utf-8").splitlines() if line.strip()]
    result = dict(rows)
    for name, version in rows:
        if result[name] != version:
            raise ValueError(f"conflicting bundled package versions: {name}")
    return result


def tree_fingerprint(distribution):
    rows = [(p.relative_to(distribution).as_posix(), digest(p.read_bytes())) for p in sorted(distribution.rglob("*")) if p.is_file() and not p.name.endswith("vendor-sha256")]
    return digest(json.dumps(rows, separators=(",", ":")).encode())


def inspect_source(path, expected_base, expected_version):
    names = archive_names(path)
    recipes = [n for n in names if n.endswith("/PKGBUILD") or n == "PKGBUILD"]
    infos = [n for n in names if n.endswith("/.SRCINFO") or n == ".SRCINFO"]
    if len(recipes) != 1 or len(infos) != 1:
        raise ValueError(f"not a complete makepkg source package: {path.name}")
    info = archive_read(path, infos[0]).decode("utf-8")
    values = lambda key: re.findall(rf"^\s*{key}\s*=\s*(.+)$", info, re.M)
    base, version, release = values("pkgbase"), values("pkgver"), values("pkgrel")
    if base != [expected_base] or version + release != expected_version.rsplit("-", 1):
        raise ValueError(f"source version does not match bundled package: {path.name}")
    # --allsource includes original archives or VCS source, not just a recipe
    # that would depend on a potentially disappearing third-party download.
    source_members = [n for n in names if n not in recipes + infos and not n.endswith("/")]
    if not source_members:
        raise ValueError(f"empty source payload: {path.name}")
    payload_names = {Path(n).name for n in source_members}
    payload_names.update(n.split("/")[1] for n in names if len(n.split("/")) > 1)
    for source in values("source") + values("source_x86_64") + values("source_ucrt64"):
        if "::" in source:
            filename = source.split("::", 1)[0]
        elif "://" in source:
            filename = Path(urllib.parse.urlsplit(source).path).name
            if source.startswith(("git+", "hg+", "svn+")):
                filename = filename.removesuffix(".git")
        else:
            filename = source
        if filename not in payload_names:
            raise ValueError(f"upstream source omitted from --allsource archive {path.name}: {filename}")
    return {"base": expected_base, "version": expected_version, "licenses": values("license"), "source_info": info}


def resolve_source(name, version, package):
    if (name, version) == ("mingw-w64-ucrt-x86_64-xpdf-tools", "4.06-1"):
        return reconstruct_xpdf()
    base = package.get("BASE", [re.sub(r"^mingw-w64-(?:ucrt-|clang-)?(?:x86_64|i686|aarch64)-", "mingw-w64-", name)])[0]
    normalized_version = version.replace("~", ":").rsplit(":", 1)[-1]
    stems = [f"{base}-{v}.src.tar" for v in dict.fromkeys([version, normalized_version])]
    urls = []
    try:
        mapping = download(f"https://raw.githubusercontent.com/git-for-windows/pacman-repo/x86_64/{name}.versions.json", CACHE / "git-metadata" / f"{name}.versions.json")
        mapping_data = json.loads(mapping.read_text(encoding="utf-8"))
        tag = mapping_data.get(version) or mapping_data.get(normalized_version)
        if tag:
            urls += [f"https://github.com/git-for-windows/pacman-repo/releases/download/{tag}/{stem}.{extension}" for stem in stems for extension in ("gz", "xz", "zst")]
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
    repository = "mingw" if name.startswith("mingw-w64-") else "msys"
    urls += [f"https://repo.msys2.org/{repository}/sources/{stem}.{extension}" for stem in stems for extension in ("zst", "xz", "gz")]
    errors = []
    for url in urls:
        path = CACHE / "git-source-packages" / url.rsplit("/", 1)[1]
        try:
            download(url, path, maximum=600_000_000)
            info = inspect_source(path, base, normalized_version)
            print(f"Git source: {name} {version} -> {path.name}", flush=True)
            return {"name": name, "version": version, "base": base, "archive": path.name, "sha256": digest(path.read_bytes()), "url": url, "licenses": info["licenses"]}
        except urllib.error.HTTPError as error:
            if error.code not in (404, 410):
                raise
            errors.append(str(error))
    raise ValueError(f"exact source unavailable: {name} {version} (base {base})")


def refresh(distribution):
    specs = [
        ("msys", "https://repo.msys2.org/msys/x86_64/msys.db"),
        ("ucrt64", "https://repo.msys2.org/mingw/ucrt64/ucrt64.db"),
        ("git-for-windows", "https://raw.githubusercontent.com/git-for-windows/pacman-repo/x86_64/git-for-windows-x86_64.db"),
    ]
    packages = {}
    for name, url in specs:
        packages.update(database(download(url, CACHE / f"{name}.db")))
    rows, errors = [], []
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        futures = {pool.submit(resolve_source, name, version, packages.get(name, {})): name for name, version in versions(distribution).items()}
        for future in concurrent.futures.as_completed(futures):
            try:
                rows.append(future.result())
            except Exception as error:
                errors.append(f"{futures[future]}: {error}")
    if errors:
        raise ValueError("Incomplete Portable Git source set:\n" + "\n".join(sorted(errors)))
    spec = json.loads((ROOT / "tools/vendors.json").read_text())["git-windows-x64"]
    reconstruct_xpdf(distribution)
    json_file(LOCK, {"version": 1, "portable_git_sha256": spec["sha256"], "tree_sha256": tree_fingerprint(distribution), "packages": sorted(rows, key=lambda p: p["name"])})


def bundle(distribution, output):
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    spec = json.loads((ROOT / "tools/vendors.json").read_text())["git-windows-x64"]
    expected = {p["name"]: p["version"] for p in lock["packages"]}
    if expected != versions(distribution) or lock["portable_git_sha256"] != spec["sha256"] or tree_fingerprint(distribution) != lock["tree_sha256"]:
        raise ValueError("Portable Git binaries or package versions do not match the reviewed source lock")
    unique = {p["archive"]: p for p in lock["packages"]}
    for record in unique.values():
        if record.get("reconstructed"):
            if reconstruct_xpdf(distribution)["sha256"] != record["sha256"]:
                raise ValueError("reconstructed Xpdf source no longer matches the reviewed checksum")
        else:
            download(record["url"], CACHE / "git-source-packages" / record["archive"], record["sha256"], maximum=600_000_000)
    with zipfile.ZipFile(output, "w", zipfile.ZIP_STORED) as archive:
        archive.writestr("PACKAGE_SOURCES.json", json.dumps(lock, indent=2) + "\n")
        archive.writestr("SOURCE_CODE.md", "# Portable Git corresponding source materials\n\nThis source kit accompanies the exact Portable Git tree identified in PACKAGE_SOURCES.json.\nEach source package contains its original PKGBUILD, .SRCINFO and upstream materials.\nUnpack the component source archive to inspect its licenses, patches and build recipe.\nBuild environment and instructions: https://gitforwindows.org/technical-overview.html\nhttps://gitforwindows.org/package-management.html\nDo not substitute newer versions when reproducing the distributed binaries.\n")
        for name in sorted(unique):
            archive.write(CACHE / "git-source-packages" / name, "packages/" + name)
    verify_kit(output)
    return {"filename": output.name, "sha256": digest(output.read_bytes()), "packages": len(expected)}


def verify_kit(path):
    import hashlib
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    with zipfile.ZipFile(path) as archive:
        if len(archive.namelist()) != len(set(archive.namelist())):
            raise ValueError("duplicate entries in source kit")
        if json.loads(archive.read("PACKAGE_SOURCES.json")) != lock:
            raise ValueError("source-kit component manifest does not match the reviewed lock")
        records = {p["archive"]: p for p in lock["packages"]}
        actual = {n for n in archive.namelist() if n.startswith("packages/")}
        if actual != {"packages/" + name for name in records}:
            raise ValueError("source-kit archives missing or unexpected")
        if set(archive.namelist()) != actual | {"PACKAGE_SOURCES.json", "SOURCE_CODE.md"}:
            raise ValueError("source-kit instructions missing or unexpected material")
        for name, record in records.items():
            hasher = hashlib.sha256()
            with archive.open("packages/" + name) as stream:
                while block := stream.read(1024 * 1024):
                    hasher.update(block)
            if hasher.hexdigest() != record["sha256"]:
                raise ValueError(f"corresponding source changed: {name}")
    print(f"Portable Git source kit verified: {len(lock['packages'])} package versions, {len(records)} archives", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["refresh", "verify"])
    parser.add_argument("distribution", type=Path)
    args = parser.parse_args()
    if args.command == "refresh":
        refresh(args.distribution)
    else:
        verify_kit(args.distribution)

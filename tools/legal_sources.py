"""Pinned runtime sources and notices, shared by packaging and CI."""
import argparse
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shlex
import shutil
import subprocess
import tarfile

from license_bundle import CACHE, ROOT, digest, download

SBCL_VERSION = "2.6.9"
SBCL_SOURCE_URL = f"https://codeload.github.com/sbcl/sbcl/tar.gz/refs/tags/sbcl-{SBCL_VERSION}"
SBCL_SOURCE_SHA256 = "cec7075066ad72b53bd72b6306e65edd72986079de1bf9be39aa60c112c0780a"
BOOTSTRAP_VERSION = "2.4.0"
BOOTSTRAP_SHA256 = "c7cb073c51f69be29edb3bb2eca7661c42a553b4a32ade880bbc8495596264a2"


def sbcl_source():
    return download(SBCL_SOURCE_URL, CACHE / f"sbcl-{SBCL_VERSION}.tar.gz", SBCL_SOURCE_SHA256)


def require_sbcl_version(version):
    if version.strip() != f"SBCL {SBCL_VERSION}":
        raise ValueError(f"runtime {version!r} has not been reviewed; expected SBCL {SBCL_VERSION}. Run tools/legal_sources.py prepare-sbcl first.")


def bundle_sbcl(destination, version):
    require_sbcl_version(version)
    archive_path = sbcl_source()
    notices = [f"Steel Bank Common Lisp {SBCL_VERSION}", f"Exact source: {SBCL_SOURCE_URL}", f"SHA-256: {SBCL_SOURCE_SHA256}", ""]
    with tarfile.open(archive_path, "r:gz") as archive:
        members = {PurePosixPath(*PurePosixPath(m.name).parts[1:]).as_posix(): m for m in archive.getmembers() if m.isfile()}
        for name in sorted(members):
            if name == "COPYING" or (name.startswith("contrib/") and PurePosixPath(name).name in ("COPYING", "LICENSE")):
                notices += [f"===== {name} =====", archive.extractfile(members[name]).read().decode("utf-8"), ""]
        rt = archive.extractfile(members["contrib/sb-rt/rt.lisp"]).read().decode("utf-8")
        header = rt.split("|#", 1)[0] + "|#"
        if "Copyright 1990" not in header or "permission" not in header.lower():
            raise ValueError("SB-RT license header needs review")
        notices += ["===== contrib/sb-rt/rt.lisp original notice =====", header, ""]
        asdf = archive.extractfile(members["contrib/asdf/asdf.texinfo"]).read().decode("utf-8")
        copying = re.search(r"@copying\s*(.*?)@end copying", asdf, re.S)
        if not copying or "Daniel Barlow" not in copying[1]:
            raise ValueError("ASDF/UIOP copyright notice needs review")
        notices += ["===== ASDF and UIOP =====", copying[1], ""]
        # Preserve each contrib's declared authors and license, including SB-SIMD.
        for name in sorted(members):
            if name.startswith("contrib/") and name.endswith(".asd"):
                text = archive.extractfile(members[name]).read().decode("utf-8")
                if re.search(r":licen[cs]e|:author|:copyright", text, re.I):
                    notices += [f"===== Original component declaration: {name} =====", text, ""]
        # Runtime/C sources can include third-party notices not listed in the
        # top-level COPYING (e.g. imported hash implementations). Preserve the
        # complete original comment blocks rather than guessing their license.
        for name in sorted(members):
            if PurePosixPath(name).suffix.lower() not in (".c", ".h", ".lisp", ".s", ".asm"):
                continue
            data = archive.extractfile(members[name]).read()
            try:
                text = data.decode("utf-8")
            except UnicodeDecodeError:
                text = data.decode("latin-1")
            blocks = re.findall(r"/\*.*?\*/|#\|.*?\|#|(?:^;[^\n]*\n)+", text, re.S | re.M)
            selected = [b for b in blocks if re.search(r"copyright|permission is hereby|redistribution and use", b, re.I)]
            if selected:
                notices += [f"===== Original notices in {name} =====", "\n\n".join(selected), ""]
    standard_mit = "Permission is hereby granted" + (ROOT / "LICENSE").read_text(encoding="utf-8").split("Permission is hereby granted", 1)[1]
    notices += ["===== Standard MIT terms for components declaring MIT above =====", standard_mit]
    (destination / "licenses/SBCL-NOTICES.txt").write_text("\n".join(notices), encoding="utf-8")
    target = destination / f"sources/sbcl-{SBCL_VERSION}.tar.gz"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(archive_path, target)
    return f"## Steel Bank Common Lisp {SBCL_VERSION}\n\nOriginal runtime and contrib notices: `licenses/SBCL-NOTICES.txt`.\nComplete upstream sources: `sources/{target.name}`.\nWindows UTF-8 resource patch, when applied: https://github.com/sherhn/Light/blob/main/tools/windows_manifest.py\n\n"


def build_sbcl(version, source, prefix, host="sbcl"):
    sources = ROOT / f"target/vendor/sbcl-{version}-build"
    sources.mkdir(parents=True, exist_ok=True)
    with tarfile.open(source, "r:gz") as archive:
        archive.extractall(sources, filter="data")
    project = sources / f"sbcl-sbcl-{version}"
    # A failed cross-compilation must stop, never enter an interactive debugger.
    command = f"{shlex.quote(str(host))} --disable-debugger --no-sysinit --no-userinit"
    subprocess.run(["sh", "make.sh", f"--prefix={prefix}", f"--xc-host={command}"], cwd=project, check=True)
    subprocess.run(["sh", "install.sh"], cwd=project, check=True)
    return prefix / "bin/sbcl"


def prepare_sbcl():
    """Build the reviewed runtime into the project; never install system-wide."""
    prefix = ROOT / f"target/vendor/sbcl-{SBCL_VERSION}"
    if platform.system() == "Windows":
        executable = prefix / "sbcl.exe"
        if not executable.is_file():
            executable = Path(r"C:\Program Files\Steel Bank Common Lisp\sbcl.exe")
        if not executable.is_file():
            raise ValueError(f"install SBCL {SBCL_VERSION}, e.g. choco install sbcl --version={SBCL_VERSION}")
        require_sbcl_version(subprocess.check_output([str(executable), "--version"], text=True).strip())
        result = executable.parent
    else:
        executable = prefix / "bin/sbcl"
        if not executable.is_file():
            host = "sbcl"
            installed = subprocess.check_output([host, "--version"], text=True).strip()
            match = re.fullmatch(r"SBCL (\d+)\.(\d+)\.(\d+).*", installed)
            if not match:
                raise ValueError(f"cannot identify bootstrap compiler: {installed}")
            if tuple(map(int, match.groups())) < (2, 4, 0):
                # Ubuntu 22.04's SBCL 2.1.11 cannot cross-compile 2.6.9 directly.
                # Build an intermediate from pinned original source; it is a
                # build tool only and is never copied into release packages.
                bootstrap_prefix = ROOT / f"target/vendor/sbcl-bootstrap-{BOOTSTRAP_VERSION}"
                host = bootstrap_prefix / "bin/sbcl"
                if not host.is_file():
                    source = download(f"https://codeload.github.com/sbcl/sbcl/tar.gz/refs/tags/sbcl-{BOOTSTRAP_VERSION}", CACHE / f"sbcl-{BOOTSTRAP_VERSION}.tar.gz", BOOTSTRAP_SHA256)
                    build_sbcl(BOOTSTRAP_VERSION, source, bootstrap_prefix)
            build_sbcl(SBCL_VERSION, sbcl_source(), prefix, host)
        require_sbcl_version(subprocess.check_output([str(executable), "--version"], text=True).strip())
        result = prefix
    if os.environ.get("GITHUB_ENV"):
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as stream:
            stream.write(f"LIGHT_BUNDLED_SBCL_DIR={result}\n")
            stream.write(f"LIGHT_SBCL={executable}\n")
    print(result, flush=True)
    return result


def native_library_notice(origin, resources):
    """Fail closed when a newly bundled Homebrew dylib has no reviewed notice."""
    match = re.search(r"/Cellar/([^/]+)/([^/]+)/", origin.as_posix())
    if not match or (match[1], match[2]) != ("zstd", "1.5.7"):
        raise ValueError(f"unreviewed native library {origin}; add its exact license and source provenance before shipping")
    license_file = ROOT / "assets/licenses/Zstandard-1.5.7-BSD.txt"
    target = resources / "licenses" / license_file.name
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(license_file, target)
    notices = resources / "THIRD_PARTY_NOTICES.md"
    with notices.open("a", encoding="utf-8") as stream:
        stream.write("\n## Zstandard 1.5.7\n\nCopyright (c) Meta Platforms, Inc. and affiliates.\nBSD-3-Clause; full text: licenses/Zstandard-1.5.7-BSD.txt.\nSource: https://github.com/facebook/zstd/tree/v1.5.7\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare-sbcl"])
    args = parser.parse_args()
    prepare_sbcl()

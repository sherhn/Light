#!/usr/bin/env python3
"""Build a self-contained Light distribution that works right after unpacking:
the app, the CLI, a bundled SBCL (REPL, compiler, ASDF), the registry of modules and all licence notices.

Usage:
  python tools/package.py [--installers] [--sbcl-dir DIR] [--no-zip] [--skip-build]

--sbcl-dir  folder with sbcl[.exe], sbcl.core and contrib/, or an SBCL install prefix (bin/, lib/sbcl/);
            default: the sbcl found on PATH.
Result: target/dist/Light-<version>-<os>-<arch>[.zip on Windows, .tar.gz elsewhere]
--installers also creates a Windows Setup.exe, macOS .dmg or Debian/Ubuntu .deb.
Windows installers include Portable Git; --git-dir can reuse an extracted copy.
"""
import argparse
import json
import os
import pathlib
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""


def version():
    m = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.M)
    return m.group(1) if m else "0.0.0"


def sbcl_parts(d):
    """(runtime, core, contrib) of an SBCL laid out as one folder (the Windows installer) or as a prefix (bin/, lib/sbcl/)."""
    for exe, home in ((d / f"sbcl{EXE}", d), (d / "bin" / f"sbcl{EXE}", d / "lib" / "sbcl")):
        if exe.is_file() and (home / "sbcl.core").is_file() and (home / "contrib").is_dir():
            return exe, home / "sbcl.core", home / "contrib"
    return None


def sbcl_from_running(exe):
    """Ask an SBCL where its runtime, core and contribs are: apt and Homebrew keep them in different folders."""
    expr = '(format t "~a~%~a~%~a" (sb-ext:native-namestring sb-ext:*runtime-pathname*) (sb-ext:native-namestring sb-ext:*core-pathname*) (sb-ext:native-namestring (sb-int:sbcl-homedir-pathname)))'
    out = subprocess.run([exe, "--noinform", "--no-sysinit", "--no-userinit", "--non-interactive", "--eval", expr], capture_output=True, text=True).stdout.splitlines()
    if len(out) == 3:
        runtime, core, home = pathlib.Path(out[0]), pathlib.Path(out[1]), pathlib.Path(out[2])
        if runtime.is_file() and core.is_file() and (home / "contrib").is_dir():
            return runtime, core, home / "contrib"
    return None


def find_sbcl(explicit):
    explicit = explicit or os.environ.get("LIGHT_BUNDLED_SBCL_DIR")
    dirs = [pathlib.Path(explicit)] if explicit else []
    exe = shutil.which("sbcl")
    if exe:
        dirs.append(pathlib.Path(exe).resolve().parent)
    dirs.append(pathlib.Path(r"C:\Program Files\Steel Bank Common Lisp"))
    for d in dirs:
        found = sbcl_parts(d)
        if found:
            return found
    found = exe and sbcl_from_running(exe)
    if found:
        return found
    sys.exit("No SBCL with sbcl, sbcl.core and contrib/ found. Install one or pass --sbcl-dir.")


def notices(destination, sbcl_version):
    from license_bundle import bundle_cargo, bundle_static
    from legal_sources import bundle_sbcl
    return ("# Third-party notices\n\nLight's original code is MIT licensed; see LICENSE.\n"
            "Third-party components retain the separate licenses and copyrights collected below.\n\n"
            + bundle_static(destination) + "\n" + bundle_sbcl(destination, sbcl_version)
            + bundle_cargo(destination))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sbcl-dir")
    ap.add_argument("--no-zip", action="store_true")
    ap.add_argument("--skip-build", action="store_true")
    ap.add_argument("--installers", action="store_true")
    ap.add_argument("--makensis", help="path to the NSIS compiler on Windows")
    ap.add_argument("--git-dir", help="an extracted Portable Git distribution (Windows)")
    a = ap.parse_args()

    if not a.skip_build:
        subprocess.run(["cargo", "build", "--locked", "--release", "-p", "light", "-p", "light-cli"], cwd=ROOT, check=True)
    sbcl_exe, sbcl_core, sbcl_contrib = find_sbcl(a.sbcl_dir)
    ver = version()
    arch = {"amd64": "x64", "x86_64": "x64", "aarch64": "arm64"}.get(platform.machine().lower(), platform.machine().lower())
    name = f"Light-{ver}-{'macos' if platform.system() == 'Darwin' else platform.system().lower()}-{arch}"
    dist = ROOT / "target" / "dist" / name
    from installers import reset_directory
    dist = reset_directory(dist, ROOT / "target/dist")
    (dist / "lisp").mkdir(parents=True)
    (dist / "licenses").mkdir()

    for exe in ("light", "light-cli"):
        src = ROOT / "target" / "release" / f"{exe}{EXE}"
        if not src.is_file():
            sys.exit(f"missing {src}; build first")
        shutil.copy2(src, dist / src.name)

    shutil.copy2(sbcl_exe, dist / "lisp" / f"sbcl{EXE}")
    shutil.copy2(sbcl_core, dist / "lisp" / "sbcl.core")
    shutil.copytree(sbcl_contrib, dist / "lisp" / "contrib")
    if os.name == "nt":
        from windows_manifest import enable_utf8
        enable_utf8(dist / "lisp/sbcl.exe")
    sbcl_ver = subprocess.run([str(sbcl_exe), "--version"], capture_output=True, text=True).stdout.strip() or "unknown"
    third_party = notices(dist, sbcl_ver)
    if a.installers and os.name == "nt":
        from installers import bundle_windows_git
        third_party += bundle_windows_git(dist, arch, a.git_dir)
    (dist / "THIRD_PARTY_NOTICES.md").write_text(third_party, encoding="utf-8")
    shutil.copy2(ROOT / "README.md", dist / "README.md")
    shutil.copy2(ROOT / "LICENSE", dist / "LICENSE")
    # the modules (languages, run commands, project templates, covers) that can be installed without a network
    registry = ROOT / "registry"
    if not (registry / "index.json").is_file():
        sys.exit("no registry/index.json: run `python tools/build_registry.py --covers` first")
    shutil.copytree(registry, dist / "registry")
    from license_bundle import seal, verify
    seal(dist)
    verify(dist)
    (dist / "START_HERE.txt").write_text(
        "Light - a modular IDE for any language, written in Rust\n\n"
        f"Run {'light.exe' if os.name == 'nt' else './light'}. Nothing else has to be installed:\n"
        "the languages come as modules - open the Modules tab (Ctrl+Alt+P) and install what you work with\n"
        "(JavaScript, Docker/Compose, Lisp, Python, C/C++, HTML, CSS, Markdown, reStructuredText, Shell and make). The folder 'registry'\n"
        "holds them, so this works offline too; the list is also read from the published registry.\n"
        "The Lisp module's REPL, compiler and ASDF come from the bundled SBCL in the 'lisp' folder.\n"
        + ("Git is included. An existing Git on your PATH takes priority.\n\n" if (dist / "git").is_dir()
           else "Git features and 'Get from Version Control' use the git on your PATH (https://git-scm.com).\n\n")
        + f"{sbcl_ver} is bundled; its original notices are in licenses/SBCL-NOTICES.txt and sources are in sources/.\n"
        + (
            "\nThe build has no Apple Developer ID signature or notarization.\n"
            "After copying Light.app to Applications, macOS may require approval in System Settings > Privacy & Security.\n"
            if platform.system() == "Darwin"
            else ""
        ),
        encoding="utf-8",
    )

    size = sum(f.stat().st_size for f in dist.rglob("*") if f.is_file()) / 1e6
    print(f"built {dist}  ({size:.0f} MB, {sbcl_ver})")
    if not a.no_zip:
        # a zip for Windows; a tarball elsewhere, since it keeps the executable bits
        archive = dist.parent / (f"{name}.zip" if os.name == "nt" else f"{name}.tar.gz")
        if archive.exists():
            archive.unlink()
        if os.name == "nt":
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
                for f in sorted(dist.rglob("*")):
                    if f.is_file():
                        z.write(f, pathlib.Path(name) / f.relative_to(dist))
        else:
            def anonymous(info):
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                return info

            with tarfile.open(archive, "w:gz", compresslevel=9) as t:
                t.add(dist, arcname=name, filter=anonymous)
        print(f"archive: {archive}  ({archive.stat().st_size / 1e6:.0f} MB)")

    if a.installers:
        from installers import build
        build(dist, ver, arch, a.makensis)


if __name__ == "__main__":
    main()

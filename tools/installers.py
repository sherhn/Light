#!/usr/bin/env python3
"""Native installers for the self-contained directory produced by package.py."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import subprocess
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
UNINSTALL_KEY = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Light"


def reset_directory(path, parent):
    path, parent = path.resolve(), parent.resolve()
    if path == parent or not path.is_relative_to(parent):
        raise ValueError(f"refusing to remove {path}: outside {parent}")
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    return path


def bundle_windows_git(dist, architecture, explicit=None):
    spec = json.loads((ROOT / "tools/vendors.json").read_text())[f"git-windows-{architecture}"]
    if explicit:
        source = Path(explicit).resolve()
    else:
        cache = ROOT / "target/vendor"
        cache.mkdir(parents=True, exist_ok=True)
        archive = cache / spec["url"].rsplit("/", 1)[1]
        if not archive.is_file() or hashlib.sha256(archive.read_bytes()).hexdigest() != spec["sha256"]:
            temporary = archive.with_suffix(".download")
            print(f"Downloading Git for Windows {spec['version']}...", flush=True)
            with urllib.request.urlopen(spec["url"], timeout=120) as response, temporary.open("wb") as out:
                shutil.copyfileobj(response, out)
            if hashlib.sha256(temporary.read_bytes()).hexdigest() != spec["sha256"]:
                temporary.unlink()
                raise ValueError("Git for Windows SHA-256 mismatch")
            temporary.replace(archive)
        source = cache / f"git-{spec['version']}-{architecture}"
        marker = source / ".light-vendor-sha256"
        if not marker.is_file() or marker.read_text() != spec["sha256"]:
            reset_directory(source, cache)
            choices = [os.environ.get("SEVENZIP"), shutil.which("7z"), shutil.which("7za")]
            if os.environ.get("ProgramFiles"):
                choices.append(str(Path(os.environ["ProgramFiles"]) / "7-Zip/7z.exe"))
            extractor = next((p for p in choices if p and Path(p).is_file()), None)
            if not extractor:
                raise ValueError("7-Zip is required to prepare Portable Git; install it, set SEVENZIP or pass --git-dir")
            subprocess.run([extractor, "x", "-y", "-bsp0", f"-o{source}", str(archive)], check=True)
            if not (source / "cmd/git.exe").is_file() or not (source / "LICENSE.txt").is_file():
                raise ValueError("incomplete Portable Git extraction")
            marker.write_text(spec["sha256"])
    if not (source / "cmd/git.exe").is_file() or not (source / "LICENSE.txt").is_file():
        raise ValueError(f"not a complete Portable Git distribution: {source}")
    shutil.copytree(source, dist / "git", ignore=shutil.ignore_patterns(".light-vendor-sha256"))
    return f"\n## Git for Windows {spec['version']}\n\nBundled in `git/`; licences in `git/LICENSE.txt`.\nSource and source packages: {spec['source']}\n"


def nsis_quote(value):
    return str(value).replace("$", "$$").replace('"', '$\\"')


def uninstall_manifest(dist):
    lines = []
    for path in sorted(dist.rglob("*")):
        if path.is_file():
            name = nsis_quote(str(path.relative_to(dist)).replace("/", "\\"))
            lines.append(f'Delete "$INSTDIR\\{name}"')
    directories = sorted((p for p in dist.rglob("*") if p.is_dir()), key=lambda p: len(p.parts), reverse=True)
    for path in directories:
        name = nsis_quote(str(path.relative_to(dist)).replace("/", "\\"))
        lines.append(f'RMDir "$INSTDIR\\{name}"')
    return "\n".join(lines) + "\n"


def windows(dist, version, architecture, compiler=None):
    candidates = [compiler, os.environ.get("MAKENSIS"), shutil.which("makensis")]
    for variable in ("ProgramFiles(x86)", "ProgramFiles"):
        if os.environ.get(variable):
            candidates.append(str(Path(os.environ[variable]) / "NSIS/makensis.exe"))
    compiler = next((p for p in candidates if p and Path(p).is_file()), None)
    if compiler is None:
        raise ValueError("NSIS is required: install NSIS or pass --makensis PATH")
    output = dist.parent / f"{dist.name}-Setup.exe"
    manifest = dist.parent / f".{dist.name}-uninstall.nsh"
    manifest.write_text(uninstall_manifest(dist), encoding="utf-8")
    definitions = {
        "APP_VERSION": version, "OUTPUT_FILE": output, "PAYLOAD_DIR": dist,
        "APP_ICON": ROOT / "assets/icon/light.ico", "UNINSTALL_KEY": UNINSTALL_KEY,
        "UNINSTALL_MANIFEST": manifest,
        "PAYLOAD_KIB": sum(p.stat().st_size for p in dist.rglob("*") if p.is_file()) // 1024,
    }
    try:
        subprocess.run([str(compiler), "/V2", "/INPUTCHARSET", "UTF8"] + [f"/D{k}={v}" for k, v in definitions.items()] + [str(ROOT / "tools/installers/light.nsi")], check=True)
    finally:
        manifest.unlink(missing_ok=True)
    return output


def desktop_entry():
    return """[Desktop Entry]
Type=Application
Name=Light
Comment=A modular IDE for any language
Exec=/opt/light/light %F
Icon=light
Terminal=false
Categories=Development;IDE;
StartupNotify=true
MimeType=text/plain;
"""


def linux(dist, version, architecture):
    arch = {"x64": "amd64", "arm64": "arm64"}[architecture]
    output = dist.parent / f"{dist.name}.deb"
    with tempfile.TemporaryDirectory(prefix="light-deb-", dir=dist.parent) as temp:
        work = Path(temp)
        root = work / "root"
        payload = root / "opt/light"
        shutil.copytree(dist, payload)
        for name in ("light", "light-cli", "lisp/sbcl"):
            (payload / name).chmod(0o755)
        for name in ("light", "light-cli"):
            link = root / "usr/bin" / name
            link.parent.mkdir(parents=True, exist_ok=True)
            link.symlink_to(f"/opt/light/{name}")
        applications = root / "usr/share/applications"
        applications.mkdir(parents=True)
        (applications / "light.desktop").write_text(desktop_entry(), encoding="utf-8")
        for size in (16, 32, 48, 128, 256, 512):
            icon = root / f"usr/share/icons/hicolor/{size}x{size}/apps/light.png"
            icon.parent.mkdir(parents=True)
            shutil.copy2(ROOT / f"assets/icon/light-{size}.png", icon)
        # Let dpkg determine the ABI versions required by this build, including SBCL.
        debian = work / "debian"
        debian.mkdir()
        (debian / "control").write_text("Source: light-ide\n\nPackage: light-ide\nArchitecture: any\nDescription: Light IDE\n")
        generated = subprocess.check_output(["dpkg-shlibdeps", "-O", f"-e{payload / 'light'}", f"-e{payload / 'light-cli'}", f"-e{payload / 'lisp/sbcl'}"], cwd=work, text=True)
        dependencies = generated.strip().removeprefix("shlibs:Depends=").split(", ")
        dependencies += ["libgl1", "libx11-6", "libxcursor1", "libxi6", "libxrandr2", "libxkbcommon0", "libwayland-client0", "libwayland-cursor0", "libwayland-egl1", "xdg-utils", "curl", "git"]
        control = root / "DEBIAN"
        control.mkdir()
        installed_size = sum(p.stat().st_size for p in payload.rglob("*") if p.is_file()) // 1024
        (control / "control").write_text(
            f"Package: light-ide\nVersion: {version}\nArchitecture: {arch}\nSection: devel\nPriority: optional\n"
            f"Maintainer: Light contributors <noreply@github.com>\nInstalled-Size: {installed_size}\n"
            f"Depends: {', '.join(dict.fromkeys(dependencies))}\nHomepage: https://github.com/sherhn/Light\n"
            "Description: Light, a modular IDE for any language\n Includes the editor, CLI, offline language module registry and bundled runtime.\n",
            encoding="utf-8",
        )
        # Package permissions must not depend on the builder's umask or checkout.
        for path in root.rglob("*"):
            if not path.is_symlink():
                executable = path.is_dir() or bool(path.stat().st_mode & stat.S_IXUSR)
                path.chmod(0o755 if executable else 0o644)
        subprocess.run(["dpkg-deb", "--root-owner-group", "--build", str(root), str(output)], check=True)
    return output


def macos_libraries(contents):
    """Relocate Homebrew libraries; nothing in the app may depend on the build machine."""
    frameworks = contents / "Frameworks"
    frameworks.mkdir()
    binaries = contents / "MacOS"
    executables = [binaries / "light", binaries / "light-cli", contents / "Resources/lisp/sbcl"]
    pending = [(p, p) for p in executables]
    copied = {}
    while pending:
        source, destination = pending.pop(0)
        destination.chmod(destination.stat().st_mode | stat.S_IWUSR)
        dependencies = subprocess.check_output(["otool", "-L", str(source)], text=True).splitlines()[1:]
        for line in dependencies:
            dependency = line.strip().rsplit(" (", 1)[0]
            if dependency.startswith(("/usr/lib/", "/System/Library/")):
                continue
            if dependency.startswith("@loader_path/"):
                origin = source.parent / dependency.removeprefix("@loader_path/")
            elif dependency.startswith("@rpath/"):
                load_commands = subprocess.check_output(["otool", "-l", str(source)], text=True)
                paths = re.findall(r"cmd LC_RPATH\s+cmdsize \d+\s+path (.+?) \(offset", load_commands)
                origins = [Path(p.replace("@loader_path", str(source.parent))) / dependency.removeprefix("@rpath/") for p in paths]
                origin = next((p for p in origins if p.is_file()), Path("/missing-library"))
            else:
                origin = Path(dependency)
            origin = origin.resolve()
            if origin == source.resolve():
                continue  # a dylib's own install name
            if not origin.is_file():
                raise ValueError(f"cannot relocate {dependency}, required by {source}")
            if origin not in copied:
                target = frameworks / origin.name
                if target.exists():
                    raise ValueError(f"library name collision: {target.name}")
                shutil.copy2(origin, target)
                copied[origin] = target
                pending.append((origin, target))
            relative = os.path.relpath(copied[origin], destination.parent)
            subprocess.run(["install_name_tool", "-change", dependency, f"@loader_path/{relative}", str(destination)], check=True)
        if destination.parent == frameworks:
            subprocess.run(["install_name_tool", "-id", f"@rpath/{destination.name}", str(destination)], check=True)
        # Signing a bundle's main executable signs the whole bundle. Do it last,
        # after its helper executables and private libraries have been signed.
        if destination != binaries / "light":
            subprocess.run(["codesign", "--force", "--sign", "-", str(destination)], check=True)


def macos(dist, version, architecture):
    output = dist.parent / f"{dist.name}.dmg"
    with tempfile.TemporaryDirectory(prefix="light-dmg-", dir=dist.parent) as temp:
        work = Path(temp)
        image = work / "image"
        contents = image / "Light.app/Contents"
        binaries = contents / "MacOS"
        binaries.mkdir(parents=True)
        resources = contents / "Resources"
        resources.mkdir()
        for source in dist.iterdir():
            target = (binaries if source.name in ("light", "light-cli") else resources) / source.name
            if source.is_dir():
                shutil.copytree(source, target)
            else:
                shutil.copy2(source, target)
        iconset = work / "Light.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                suffix = "@2x" if scale == 2 else ""
                shutil.copy2(ROOT / f"assets/icon/light-{size * scale}.png", iconset / f"icon_{size}x{size}{suffix}.png")
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(resources / "Light.icns")], check=True)
        info = {
            "CFBundleName": "Light", "CFBundleDisplayName": "Light", "CFBundleExecutable": "light",
            "CFBundleIdentifier": "io.github.sherhn.light", "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version, "CFBundleVersion": version,
            "CFBundleIconFile": "Light.icns", "LSMinimumSystemVersion": "15.0",
            "NSHighResolutionCapable": True,
        }
        (contents / "Info.plist").write_bytes(plistlib.dumps(info))
        macos_libraries(contents)
        subprocess.run(["codesign", "--force", "--sign", "-", str(contents.parent)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(contents.parent)], check=True)
        (image / "Applications").symlink_to("/Applications", target_is_directory=True)
        (image / "Install.txt").write_text("Drag Light.app to Applications, then open Light from Applications.\n", encoding="utf-8")
        subprocess.run(["hdiutil", "create", "-volname", "Light", "-srcfolder", str(image), "-format", "UDZO", "-ov", str(output)], check=True)
    return output


def build(dist, version, architecture, compiler=None):
    import platform
    builders = {"Windows": windows, "Darwin": macos, "Linux": linux}
    builder = builders[platform.system()]
    arguments = {"compiler": compiler} if builder is windows else {}
    installer = builder(dist, version, architecture, **arguments)
    digest = hashlib.sha256(installer.read_bytes()).hexdigest()
    installer.with_name(installer.name + ".sha256").write_text(f"{digest}  {installer.name}\n", encoding="utf-8")
    print(f"installer: {installer} ({installer.stat().st_size / 1e6:.0f} MB)", flush=True)
    return installer

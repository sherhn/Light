#!/usr/bin/env python3
"""Exercise real installer payloads; Windows also verifies upgrade and uninstall.

Run on a clean build machine. Refuses to replace an existing Windows installation.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import time

from installers import UNINSTALL_KEY


def run(*args, **kwargs):
    try:
        return subprocess.run([str(a) for a in args], check=True, text=True, encoding="utf-8", errors="replace", capture_output=True, timeout=120, **kwargs).stdout
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(error.stdout or "", error.stderr or "", flush=True)
        raise


def run_setup(setup, destination):
    # NSIS requires /D= to be last, with its space-containing remainder unquoted.
    # Pass a Windows command line directly, without involving a command shell.
    command = subprocess.list2cmdline([str(setup), "/S"]) + " /D=" + str(destination)
    subprocess.run(command, check=True, text=True, capture_output=True, timeout=120)


def check_payload(payload, work, gui=False, resources=None):
    windows = platform.system() == "Windows"
    suffix = ".exe" if windows else ""
    cli = payload / f"light-cli{suffix}"
    resources = resources or payload
    from license_bundle import verify
    verify(resources)
    assert (resources / "registry/index.json").is_file()
    assert (resources / "lisp" / f"sbcl{suffix}").is_file()
    env = dict(os.environ)
    for name in ("LIGHT_SBCL", "LIGHT_BUNDLE_DIR", "SBCL_HOME", "LIGHT_REGISTRY", "LIGHT_PRELOAD_MODULES"):
        env.pop(name, None)
    env["LIGHT_CONFIG_DIR"] = str(work / "config")
    env["LIGHT_GITHUB_AUTODETECT"] = "0"
    env["XDG_CACHE_HOME"] = str(work / "cache")
    if windows:
        env["PATH"] = str(Path(os.environ["SystemRoot"]) / "System32")
    assert "light-cli - headless Light engine" in run(cli, "help", env=env)
    detected = run(cli, "which", env=env).replace("\\", "/").lower()
    assert str(resources / "lisp").replace("\\", "/").lower() in detected, detected
    assert "42" in run(cli, "eval", "(+ 40 2)", env=env)
    run(cli, "eval", "(require :asdf) (asdf:asdf-version)", env=env)
    if windows:
        # No system Git on PATH: the installed CLI has to find the bundled one.
        git = payload / "git/cmd/git.exe"
        source = work / "source"
        source.mkdir(exist_ok=True)
        git_env = dict(env, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
        run(git, "credential-manager", "--version", env=git_env)
        run(git, "init", "-q", source, env=git_env)
        (source / "README.md").write_text("installer test\n")
        run(git, "-C", source, "add", "README.md", env=git_env)
        run(git, "-C", source, "-c", "user.name=Installer test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false", "commit", "-qm", "fixture", env=git_env)
        run(cli, "clone", source, work / "clone", env=git_env)
        assert (work / "clone/README.md").read_text() == "installer test\n"
    if gui:
        sample = work / "hello.py"
        sample.write_text("print('hello')\n")
        steps = [
            {"op": "assert_modules", "installed": []},
            {"op": "command", "value": "app.modules"},
            {"op": "wait", "what": "modules_index", "timeout": 30},
            {"op": "module_install", "value": "python"},
            {"op": "wait", "what": "modules_idle", "timeout": 30},
            {"op": "open", "path": str(sample)},
            {"op": "assert_language", "value": "python"},
            {"op": "quit"},
        ]
        script = work / "script.json"
        script.write_text(json.dumps(steps))
        env["LIGHT_REGISTRY"] = str(resources / "registry")
        env["LIGHT_PRELOAD_MODULES"] = ""
        output = run(payload / f"light{suffix}", "--script", script, "--no-restore", env=env)
        assert "SCRIPT OK" in output, output


def windows(setup):
    import winreg

    def registration():
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, UNINSTALL_KEY) as key:
                return winreg.QueryValueEx(key, "InstallLocation")[0]
        except FileNotFoundError:
            return None

    def folder(csidl):
        result = ctypes.create_unicode_buffer(32768)
        if ctypes.windll.shell32.SHGetFolderPathW(None, csidl, None, 0, result):
            raise RuntimeError("cannot resolve Windows shortcut directory")
        return Path(result.value)

    programs, desktop = folder(2), folder(16)
    if registration() or (programs / "Light").exists() or (desktop / "Light.lnk").exists():
        raise RuntimeError("refusing to overwrite an existing Light installation or shortcuts; use a clean test account")
    with tempfile.TemporaryDirectory(prefix="light-install-test-") as temp:
        work = Path(temp)
        install = work / "Русский путь/Light"
        preferences = work / "config/settings.json"
        preferences.parent.mkdir()
        preferences.write_text(json.dumps({"font_size": 17.0}), encoding="utf-8")
        try:
            print("Installing into a Unicode path...", flush=True)
            run_setup(setup, install)
            assert registration() == str(install), (registration(), str(install))
            assert (programs / "Light/Light.lnk").is_file()
            print("Checking bundled tools and offline modules...", flush=True)
            check_payload(install, work, gui=True)
            note = install / "user-note.txt"
            note.write_text("keep user files")
            saved_preferences = preferences.read_bytes()
            print("Checking upgrade...", flush=True)
            run_setup(setup, install)
            assert registration() == str(install)
            assert note.read_text() == "keep user files"
            assert preferences.read_bytes() == saved_preferences
            assert "light-cli" in run(install / "light-cli.exe", "help")
        finally:
            if registration() == str(install) and (install / "Uninstall.exe").is_file():
                print("Checking uninstall...", flush=True)
                run(install / "Uninstall.exe", "/S")
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline and (registration() is not None or (install / "light.exe").exists()):
                    time.sleep(0.1)
        assert registration() is None
        assert not (programs / "Light").exists()
        assert not (install / "light.exe").exists()
        assert not (install / "git").exists()
        assert note.read_text() == "keep user files"
        assert preferences.read_bytes() == saved_preferences
    print("PASS: install, bundled Git/SBCL, offline modules, Unicode paths, upgrade and uninstall")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("installer", type=Path)
    args = parser.parse_args()
    installer = args.installer.resolve()
    if platform.system() == "Windows":
        windows(installer)
    elif platform.system() == "Darwin":
        with tempfile.TemporaryDirectory(prefix="light-install-test-") as temp:
            work = Path(temp)
            mount = work / "mounted"
            run("hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", mount, installer)
            try:
                app = mount / "Light.app"
                run("codesign", "--verify", "--deep", "--strict", app)
                check_payload(app / "Contents/MacOS", work, resources=app / "Contents/Resources")
            finally:
                run("hdiutil", "detach", mount)
        print("PASS: disk image, app signature, relocated payload and bundled runtime")
    else:
        with tempfile.TemporaryDirectory(prefix="light-install-test-") as temp:
            work = Path(temp)
            run("dpkg-deb", "--extract", installer, work / "root")
            run("desktop-file-validate", work / "root/usr/share/applications/light.desktop")
            check_payload(work / "root/opt/light", work)
        print("PASS: Debian package, desktop entry, relocated payload and bundled runtime")


if __name__ == "__main__":
    main()

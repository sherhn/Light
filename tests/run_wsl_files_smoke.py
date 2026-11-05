"""Exercise a real WSL UNC listing, Tab completion, viewer and verified Windows copy.

Uses only a new /tmp/light-wsl-files-* folder and a TemporaryDirectory on Windows.
No installation or existing project is changed. Not part of fake-WSL CI tests.
"""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
PROFILE = "release" if "--release" in sys.argv else "debug"
EXE = ROOT / "target" / PROFILE / "light.exe"
if os.name != "nt":
    raise SystemExit("This smoke test needs Windows with a real WSL distribution")
env = {key: value for key, value in os.environ.items() if not key.startswith("LIGHT_FAKE_")}
env["WSL_UTF8"] = "1"


def wsl(*args):
    result = subprocess.run(["wsl.exe", *args], capture_output=True, env=env, timeout=60)
    if result.returncode:
        raise RuntimeError(result.stderr.decode("utf-8", errors="replace"))
    return result.stdout.decode("utf-8", errors="replace").replace("\x00", "").strip()


distros = wsl("--list", "--quiet").splitlines()
distro = os.environ.get("LIGHT_REAL_WSL_DISTRO", "Ubuntu-24.04")
assert distro in distros, f"{distro} is not installed: {distros}"
linux = wsl("-d", distro, "--exec", "mktemp", "-d", "/tmp/light-wsl-files-XXXXXX")
assert re.fullmatch(r"/tmp/light-wsl-files-[A-Za-z0-9]+", linux), linux
unc = Path(r"\\wsl.localhost") / distro / PurePosixPath(linux).relative_to("/")
assert unc.parent.name == "tmp" and unc.name.startswith("light-wsl-files-")
linked = False
try:
    source = unc / "Мои проекты"
    (source / "empty").mkdir(parents=True)
    (source / "src").mkdir()
    content = "def main():\n    print('WSL Unicode: привет')\n\nmain()\n"
    (source / "src/main.py").write_text(content, encoding="utf-8")
    (source / "notes.md").write_text("# Заметки из WSL\n", encoding="utf-8")
    (unc / "links").mkdir()
    wsl("-d", distro, "--exec", "ln", "-s", "/etc", linux + "/links/external")
    linked = True
    with tempfile.TemporaryDirectory(prefix="light-wsl-copy-test-") as directory:
        work = Path(directory)
        config = work / "config"
        config.mkdir()
        target = work / "Windows destination"
        target.mkdir()
        (config / "settings.json").write_text(json.dumps({"modules_welcome_done": True, "wsl_distro": distro, "auto_update_ide": False, "auto_update_modules": False}), encoding="utf-8")
        current = dict(env, LIGHT_CONFIG_DIR=str(config), LIGHT_REGISTRY=str(ROOT / "registry"), LIGHT_PRELOAD_MODULES="python")
        shots = ROOT / "target/e2e-shots"
        shots.mkdir(parents=True, exist_ok=True)
        script = [
            {"op": "command", "value": "file.browse_files"},
            {"op": "project_browser", "side": "linux", "go": linux},
            {"op": "wait", "what": "project_browser_idle", "timeout": 60},
            {"op": "assert_project_browser", "side": "linux", "contains": ["Мои проекты"]},
            {"op": "key", "value": "Ctrl+L"}, {"op": "frames", "value": 3},
            {"op": "key", "value": "Ctrl+A"}, {"op": "text", "value": linux + "/Мои пр"},
            {"op": "key", "value": "Tab"},
            {"op": "wait", "what": "path_input", "value": linux + "/Мои проекты/", "timeout": 30},
            {"op": "key", "value": "Enter"},
            {"op": "wait", "what": "project_browser_idle", "timeout": 30},
            {"op": "assert_project_browser", "contains": ["src", "notes.md"]},
            {"op": "project_browser", "select": "notes.md"},
            {"op": "screenshot", "path": str(shots / "real_wsl_files.png")},
            {"op": "file_browser_button", "value": "Open in IDE"},
            {"op": "assert_text_contains", "value": "Заметки из WSL"},
            {"op": "open_project", "path": str(source)},
            {"op": "wait", "what": "index", "timeout": 60},
            {"op": "open", "path": str(source / "src/main.py")},
            {"op": "frames", "value": 3},
            {"op": "gutter_icon_click", "line": 1},
            {"op": "frames", "value": 3},
            {"op": "gutter_menu_click", "value": "Run function 'main'"},
            {"op": "wait", "what": "run_done", "timeout": 60},
            {"op": "assert_run_contains", "value": "WSL Unicode: привет"},
            {"op": "terminal_mode", "value": "commands"},
            {"op": "key", "value": "Alt+F12"},
            {"op": "frames", "value": 3},
            {"op": "text", "value": "cat /etc/pas"},
            {"op": "key", "value": "Tab"},
            {"op": "wait", "what": "term_input", "value": "cat /etc/passwd ", "timeout": 30},
            {"op": "copy_file", "path": str(source), "destination": str(target)},
            {"op": "wait", "what": "file_transfer_idle", "timeout": 60},
            {"op": "assert_file_copy"},
            {"op": "screenshot", "path": str(shots / "real_wsl_copied.png")},
            {"op": "copy_file", "path": str(unc / "links"), "destination": str(target)},
            {"op": "wait", "what": "file_transfer_idle", "timeout": 60},
            {"op": "assert_file_copy", "error": "Links and special files"},
            {"op": "quit"},
        ]
        script_path = work / "script.json"
        script_path.write_text(json.dumps(script, ensure_ascii=False), encoding="utf-8")
        result = subprocess.run([str(EXE), "--script", str(script_path), "--size", "1280x800", "--no-restore"], capture_output=True, env=current, timeout=180)
        log = result.stdout.decode("utf-8", errors="replace") + result.stderr.decode("utf-8", errors="replace")
        (ROOT / f"target/real-wsl-files-{PROFILE}.log").write_text(log, encoding="utf-8")
        assert result.returncode == 0 and "SCRIPT OK" in log, log
        copied = target / source.name
        for relative in ["src/main.py", "notes.md"]:
            assert hashlib.sha256((source / relative).read_bytes()).digest() == hashlib.sha256((copied / relative).read_bytes()).digest()
        assert (copied / "empty").is_dir()
        assert not (target / "links").exists()
        before = (config / "settings.json").read_bytes()
        viewer_env = dict(current, LIGHT_VIEWER_SCREENSHOT=str(shots / "real_wsl_quick_view.png"))
        viewer = subprocess.run([str(EXE), "--viewer-smoke", str(source / "src/main.py")], capture_output=True, env=viewer_env, timeout=60)
        viewer_log = viewer.stderr.decode("utf-8", errors="replace")
        assert viewer.returncode == 0 and "QUICK VIEW READY" in viewer_log, viewer_log
        assert (config / "settings.json").read_bytes() == before
        assert (source / "src/main.py").read_text(encoding="utf-8") == content
        print(f"PASS real {distro}: UNC browser, field/terminal Tab completion, Python function button, verified folder copy, empty directories, symlink rejection and read-only quick view")
        print(viewer_log.strip())
finally:
    # The exact mktemp result was validated before any files were created.
    assert unc.parent.name == "tmp" and unc.name.startswith("light-wsl-files-")
    if linked:
        wsl("-d", distro, "--exec", "unlink", linux + "/links/external")
    shutil.rmtree(unc)

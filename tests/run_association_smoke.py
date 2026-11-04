"""Compile and exercise the Windows installer using an isolated HKCU namespace."""
import argparse
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import uuid
import winreg

parser = argparse.ArgumentParser()
parser.add_argument("--makensis", required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(root / "tools"))
import installers

namespace = "Light-Association-Smoke-" + uuid.uuid4().hex
prefix = "Software\\" + namespace + "\\"
def clean(path):
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, path, 0, winreg.KEY_ALL_ACCESS) as key:
        while True:
            try:
                child = winreg.EnumKey(key, 0)
            except OSError:
                break
            clean(path + "\\" + child)
    winreg.DeleteKey(winreg.HKEY_CURRENT_USER, path)
try:
    with tempfile.TemporaryDirectory(prefix="association-smoke-", dir=root / "target") as directory:
        work = Path(directory)
        script_root = work / "source"
        (script_root / "tools/installers").mkdir(parents=True)
        (script_root / "assets/icon").mkdir(parents=True)
        shutil.copy2(root / "assets/icon/light.ico", script_root / "assets/icon/light.ico")
        script = (root / "tools/installers/light.nsi").read_text(encoding="utf-8")
        script = script.replace("Software\\", prefix).replace("$SMPROGRAMS", str(work / "shortcuts")).replace("$DESKTOP", str(work / "desktop"))
        (script_root / "tools/installers/light.nsi").write_text(script, encoding="utf-8")
        payload = work / "payload"
        payload.mkdir()
        (payload / "light.exe").write_bytes(b"test payload; never executed")
        target = work / "installed"
        installers.ROOT = script_root
        installers.UNINSTALL_KEY = prefix + "Microsoft\\Windows\\CurrentVersion\\Uninstall\\Light"
        setup = installers.windows(payload, "1.1.0", "x64", str(Path(args.makensis).resolve()))
        subprocess.run([str(setup), "/S", "/D=" + str(target)], check=True, timeout=30)
        def value(path, name=""):
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, prefix + path) as key:
                return winreg.QueryValueEx(key, name)[0]
        command = value("Classes\\Light.Source\\shell\\open\\command")
        assert command == f'"{target}\\light.exe" --view "%1"', command
        assert value("Classes\\Applications\\light.exe\\shell\\open\\command") == command
        for extension in [".txt", ".rs", ".js", ".py", ".md", ".json", ".lisp"]:
            assert value("Classes\\" + extension + "\\OpenWithProgids", "Light.Source") == ""
            assert value("Light\\Capabilities\\FileAssociations", extension) == "Light.Source"
        assert value("RegisteredApplications", "Light") == prefix + "Light\\Capabilities"
        subprocess.run([str(target / "Uninstall.exe"), "/S", "_?=" + str(target)], check=True, timeout=30)
        try:
            value("Classes\\Light.Source\\shell\\open\\command")
            raise AssertionError("uninstall left the association command")
        except FileNotFoundError:
            pass
finally:
    try:
        clean("Software\\" + namespace)
    except FileNotFoundError:
        pass
print("PASS compiled Windows installer: quoted viewer command, Open With, capabilities and uninstall cleanup")

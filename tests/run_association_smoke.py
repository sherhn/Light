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
import file_types

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
        shutil.copytree(root / "assets/file-icons",payload / "file-icons")
        target = work / "installed"
        installers.ROOT = script_root
        installers.UNINSTALL_KEY = prefix + "Microsoft\\Windows\\CurrentVersion\\Uninstall\\Light"
        installers.REGISTRY_PREFIX = prefix.rstrip("\\")
        generated = file_types.nsis_macros(installers.REGISTRY_PREFIX)
        assert 'HKCU "Software\\Classes\\' not in generated, "test must never write real file associations"
        with winreg.CreateKey(winreg.HKEY_CURRENT_USER,prefix + "Classes\\.py") as key:
            winreg.SetValueEx(key,"",0,winreg.REG_SZ,"Other.Python")
        choice = prefix + "Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.py\\UserChoice"
        with winreg.CreateKey(winreg.HKEY_CURRENT_USER,choice) as key:
            winreg.SetValueEx(key,"ProgId",0,winreg.REG_SZ,"Other.Editor")
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
        for extension,kind in file_types.extensions().items():
            progid="Light.Source."+kind['id']
            assert value("Classes\\."+extension+"\\OpenWithProgids",progid)==""
            assert value("Light\\Capabilities\\FileAssociations","."+extension)==progid
            assert value("Classes\\"+progid+"\\shell\\open\\command")==command
            assert value("Classes\\"+progid+"\\DefaultIcon")==f'"{target}\\file-icons\\{kind["id"]}.ico",0'
            assert (target/"file-icons"/(kind['id']+".ico")).is_file()
        assert value("Classes\\.py")=="Other.Python"
        assert value("Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.py\\UserChoice","ProgId")=="Other.Editor"
        assert value("RegisteredApplications", "Light") == prefix + "Light\\Capabilities"
        subprocess.run([str(target / "Uninstall.exe"), "/S", "_?=" + str(target)], check=True, timeout=30)
        try:
            value("Classes\\Light.Source\\shell\\open\\command")
            raise AssertionError("uninstall left the association command")
        except FileNotFoundError:
            pass
        assert value("Classes\\.py")=="Other.Python"
        assert value("Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.py\\UserChoice","ProgId")=="Other.Editor"
        for kind in file_types.load():
            try:
                value("Classes\\Light.Source."+kind['id'])
                raise AssertionError("uninstall left a per-type registration")
            except FileNotFoundError:
                pass
finally:
    try:
        clean("Software\\" + namespace)
    except FileNotFoundError:
        pass
print(f"PASS compiled Windows installer: {len(file_types.extensions())} file types, distinct icon families, preserved defaults and uninstall cleanup")

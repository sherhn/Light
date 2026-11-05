#!/usr/bin/env python3
"""Exercise the real update helper in disposable installations, never the user's IDE.

With --metadata/--archive/--bootstrap, also activate a published signed package
and roll it back using a previously built updater-capable executable.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def run(executable, *args, env, success=True):
    result = subprocess.run([str(executable), *map(str, args)], env=env, capture_output=True, text=True, timeout=240)
    if success:
        assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
    else:
        assert result.returncode != 0, "invalid update was accepted"
    return result


def version(executable, env):
    result = run(executable, "--light-update-self-check", env=env)
    lines = [line.removeprefix("LIGHT_UPDATE_OK ") for line in result.stdout.splitlines() if line.startswith("LIGHT_UPDATE_OK ")]
    assert len(lines) == 1, result.stdout
    return lines[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--exe", type=Path)
    parser.add_argument("--bootstrap", type=Path)
    parser.add_argument("--metadata", type=Path)
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    executable = (args.exe or ROOT / "target" / ("release" if args.release else "debug") / ("light.exe" if os.name == "nt" else "light")).resolve()
    with tempfile.TemporaryDirectory(prefix="light-update-smoke-") as directory:
        temp = Path(directory)
        profile = temp / "profile"
        env = dict(os.environ, LIGHT_CONFIG_DIR=str(profile), LIGHT_MODULES_DIR=str(profile / "modules"), LIGHT_DISABLE_AUTO_UPDATES="1")
        current = version(executable, env)
        assert not profile.exists(), "startup self-check initialized a user profile"
        installation = temp / "Light"
        installation.mkdir()
        name = "light.exe" if os.name == "nt" else "light"
        bootstrap = (args.bootstrap or executable).resolve()
        old_version = version(bootstrap, env)
        shutil.copy2(bootstrap, installation / name)
        save(installation / "update-manifest.json", {"schema": 1, "version": old_version, "entrypoint": name, "files": {name: digest(installation / name)}})
        (installation / "personal-notes.md").write_text("Keep this note", encoding="utf-8")
        save(profile / "settings.json", {"user": "preserve preferences"})
        save(profile / "modules/state.json", {"enabled": {"python": False}})
        original_settings = (profile / "settings.json").read_bytes()
        original_modules = (profile / "modules/state.json").read_bytes()
        cache = profile / "updates"
        identity = hashlib.sha256(str(installation).encode()).hexdigest()
        pending = cache / f"pending-ide-{identity}.json"
        save(pending, {"envelope": '{"signed":"tampered","signature":"' + "00" * 64 + '"}', "destination": str(installation), "base_version": old_version})
        original_binary = digest(installation / name)
        run(executable, "--light-apply-update", pending, env=env, success=False)
        assert digest(installation / name) == original_binary
        assert "signature" in json.loads((cache / "state.json").read_text())["status"]
        print(f"Native self-check {current}; invalid signature rejected without modifying the installation", flush=True)

        if args.metadata:
            assert args.archive and args.bootstrap, "signed activation requires --archive and --bootstrap"
            envelope = args.metadata.read_text(encoding="utf-8")
            manifest = json.loads(json.loads(envelope)["signed"])
            new_version = manifest["version"]
            assert tuple(map(int, new_version.split("."))) > tuple(map(int, old_version.split(".")))
            platform = "windows-x64" if os.name == "nt" else "linux-x64"
            artifact = manifest["targets"][platform]
            archive = cache / "ide" / new_version / artifact["name"]
            archive.parent.mkdir(parents=True)
            shutil.copyfile(args.archive, archive)
            save(pending, {"envelope": envelope, "destination": str(installation), "base_version": old_version})
            run(bootstrap, "--light-apply-update", pending, env=env)
            assert version(installation / name, env) == new_version
            assert not pending.exists()
            assert (installation / "personal-notes.md").read_text() == "Keep this note"
            (installation / "after-update.md").write_text("New note after update", encoding="utf-8")
            record = json.loads((cache / f"last-install-{identity}.json").read_text())
            rollback = cache / f"rollback-ide-{identity}.json"
            save(rollback, record)
            helper = cache / name
            shutil.copy2(installation / name, helper)
            run(helper, "--light-rollback-update", rollback, env=env)
            assert version(installation / name, env) == old_version
            assert (installation / "after-update.md").read_text() == "New note after update"
            assert (installation / "personal-notes.md").read_text() == "Keep this note"
            print(f"Published signed package {old_version} → {new_version} → {old_version}: activation and rollback passed", flush=True)

        assert (profile / "settings.json").read_bytes() == original_settings
        assert (profile / "modules/state.json").read_bytes() == original_modules
        print("Preferences, disabled modules and user notes preserved", flush=True)


if __name__ == "__main__":
    main()

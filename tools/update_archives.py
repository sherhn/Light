"""Produce platform-neutral update ZIPs and an inventory of owned files."""
import hashlib
import json
from pathlib import Path
import stat
import zipfile


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            result.update(block)
    return result.hexdigest()


def inventory(root, version, entrypoint, manifest="update-manifest.json"):
    root = Path(root)
    files = {}
    for path in sorted(root.rglob("*")):
        if path.is_file() and path != root / manifest:
            files[path.relative_to(root).as_posix()] = digest(path)
    if entrypoint not in files:
        raise ValueError("update entrypoint is missing")
    value = {"schema": 1, "version": version, "entrypoint": entrypoint, "files": files}
    (root / manifest).parent.mkdir(parents=True, exist_ok=True)
    (root / manifest).write_text(json.dumps(value, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    return value


def package(root, output, version, entrypoint, refresh=True):
    root, output = Path(root), Path(output)
    if refresh:
        inventory(root, version, entrypoint)
    temporary = output.with_suffix(".zip.download")
    try:
        with zipfile.ZipFile(temporary, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
            for path in sorted(root.rglob("*")):
                if path.is_file():
                    # Read file symlinks as regular bytes; updater archives never create links.
                    info = zipfile.ZipInfo(path.relative_to(root).as_posix(), (2024, 1, 1, 0, 0, 0))
                    info.external_attr = (stat.S_IFREG | (path.stat().st_mode & 0o777)) << 16
                    info.compress_type = zipfile.ZIP_DEFLATED
                    with path.open("rb") as source, archive.open(info, "w", force_zip64=True) as target:
                        import shutil
                        shutil.copyfileobj(source, target)
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)
    return output


def verify(path):
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        if len({name.lower() for name in names}) != len(names):
            raise ValueError("duplicate update entries")
        for name in names:
            parts = name.split("/")
            if "\\" in name or ":" in name or any(part in ("", ".", "..") for part in parts):
                raise ValueError("unsafe update path")
        inventory_name = "update-manifest.json" if "update-manifest.json" in names else "Contents/_CodeSignature/light-update-manifest.json"
        value = json.loads(archive.read(inventory_name))
        expected = set(value["files"]) | {inventory_name}
        if not set(names) <= expected or not set(value["files"]) <= set(names):
            raise ValueError("update inventory has missing or extra files")
        for name, expected_hash in value["files"].items():
            info = archive.getinfo(name)
            if info.file_size > 600 * 1024 * 1024 or stat.S_ISLNK(info.external_attr >> 16):
                raise ValueError("unsafe update file")
            hasher = hashlib.sha256()
            with archive.open(name) as stream:
                while block := stream.read(1024 * 1024):
                    hasher.update(block)
            if hasher.hexdigest() != expected_hash:
                raise ValueError("update inventory hash mismatch")
    print(f"Update archive verified: {path.name}", flush=True)

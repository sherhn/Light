"""Produce platform-neutral update ZIPs and an inventory of owned files."""
import hashlib
import json
from pathlib import Path
import stat
import struct
import tempfile
import zipfile


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            result.update(block)
    return result.hexdigest()


def macho_digest(path):
    """Hash executable bytes independently of the bundle's resource-signature cycle.

    Only Apple's signature blob and its size bookkeeping are normalized. All
    executable code, data and other load commands remain covered. The signed
    release archive separately authenticates the complete final signature.
    """
    data = bytearray(path.read_bytes())
    if len(data) < 32 or struct.unpack_from('<I', data)[0] != 0xFEEDFACF:
        raise ValueError('native macOS updates require a thin 64-bit Mach-O')
    count, command_bytes = struct.unpack_from('<II', data, 16)
    end = 32 + command_bytes
    if end > len(data) or count > 10000:
        raise ValueError('invalid Mach-O load commands')
    cursor, signature = 32, None
    for _ in range(count):
        if cursor + 8 > end:
            raise ValueError('truncated Mach-O command')
        command, size = struct.unpack_from('<II', data, cursor)
        if size < 8 or cursor + size > end:
            raise ValueError('invalid Mach-O command size')
        if command == 0x1D:  # LC_CODE_SIGNATURE
            if size < 16 or signature is not None:
                raise ValueError('invalid Mach-O signature command')
            signature = struct.unpack_from('<II', data, cursor + 8)
            data[cursor+8:cursor+16] = bytes(8)
        if command == 0x19 and size >= 72 and data[cursor+8:cursor+24].rstrip(b'\0') == b'__LINKEDIT':
            data[cursor+32:cursor+40] = bytes(8)  # vmsize
            data[cursor+48:cursor+56] = bytes(8)  # filesize
        cursor += size
    if signature is None:
        raise ValueError('unsigned Mach-O executable')
    offset, size = signature
    if offset < end or size == 0 or offset + size != len(data):
        raise ValueError('invalid Mach-O signature range')
    return hashlib.sha256(data[:offset]).hexdigest()


def inventory(root, version, entrypoint, manifest="update-manifest.json"):
    root = Path(root)
    files = {}
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if path.is_file() and path != root / manifest and relative != 'Contents/_CodeSignature/CodeResources':
            files[relative] = 'macho-sha256:' + macho_digest(path) if relative == entrypoint and entrypoint.startswith('Contents/MacOS/') else digest(path)
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
        inventory_name = "update-manifest.json" if "update-manifest.json" in names else "Contents/Resources/light-update-manifest.json"
        value = json.loads(archive.read(inventory_name))
        expected = set(value["files"]) | {inventory_name}
        if value['entrypoint'].startswith('Contents/MacOS/'):
            expected.add('Contents/_CodeSignature/CodeResources')
        if not set(names) <= expected or not set(value["files"]) <= set(names):
            raise ValueError("update inventory has missing or extra files")
        for name, expected_hash in value["files"].items():
            info = archive.getinfo(name)
            if info.file_size > 600 * 1024 * 1024 or stat.S_ISLNK(info.external_attr >> 16):
                raise ValueError("unsafe update file")
            if expected_hash.startswith('macho-sha256:'):
                with tempfile.TemporaryDirectory() as temporary:
                    executable = Path(temporary) / 'executable'
                    executable.write_bytes(archive.read(name))
                    actual = 'macho-sha256:' + macho_digest(executable)
                if actual != expected_hash:
                    raise ValueError('Mach-O update inventory hash mismatch')
                continue
            hasher = hashlib.sha256()
            with archive.open(name) as stream:
                while block := stream.read(1024 * 1024):
                    hasher.update(block)
            if hasher.hexdigest() != expected_hash:
                raise ValueError("update inventory hash mismatch")
    print(f"Update archive verified: {path.name}", flush=True)

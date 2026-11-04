#!/usr/bin/env python3
"""Materialize the pinned module registry for builds, tests and offline installers."""
import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import shutil
import tempfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent.parent
LIMIT = 100 * 1024 * 1024

def digest(data):
    return hashlib.sha256(data).hexdigest()

def validate(data, expected):
    if len(data) > LIMIT or digest(data) != expected:
        raise ValueError('Pinned module snapshot checksum mismatch')
    files = {}
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        total = 0
        for item in archive.infolist():
            path = PurePosixPath(item.filename)
            total += item.file_size
            if total > LIMIT or path.is_absolute() or len(path.parts) < 2 or path.parts[0] != 'registry' or '..' in path.parts or '\\' in item.filename or ':' in item.filename or item.is_dir() or (item.external_attr >> 16) & 0o170000 == 0o120000:
                raise ValueError('Unsafe module snapshot entry')
            relative = path.relative_to('registry').as_posix()
            if relative in files:
                raise ValueError('Duplicate module snapshot entry')
            files[relative] = archive.read(item)
    index = json.loads(files['index.json'])
    expected_files = {'index.json'}
    for module in index['modules']:
        for item in module['files']:
            name = module['path'] + '/' + item['name']
            payload = files[name]
            if len(payload) != item['size'] or digest(payload) != item['sha256']:
                raise ValueError('Module file checksum mismatch: ' + name)
            expected_files.add(name)
    if expected_files != files.keys():
        raise ValueError('Snapshot contains files missing from its index')
    return files

def prepare(local=None):
    lock = json.loads((ROOT / 'modules.lock.json').read_text(encoding='utf-8'))
    cached = ROOT / 'target/module-cache' / (lock['sha256'] + '.zip')
    if local:
        data = Path(local).read_bytes()
    elif cached.is_file() and digest(cached.read_bytes()) == lock['sha256']:
        data = cached.read_bytes()
    else:
        if not lock['url'].startswith('https://'):
            raise ValueError('Pinned snapshot must use HTTPS')
        with urllib.request.urlopen(urllib.request.Request(lock['url'], headers={'User-Agent': 'Light-module-build'}), timeout=60) as response:
            data = response.read(LIMIT + 1)
    files = validate(data, lock['sha256'])
    cached.parent.mkdir(parents=True, exist_ok=True)
    cached.write_bytes(data)
    # All bytes are checked before replacing the registry; a failed download preserves it.
    registry = ROOT / 'registry'
    expected = set(files)
    current = {p.relative_to(registry).as_posix(): p for p in registry.rglob('*') if p.is_file()} if registry.exists() else {}
    if expected == current.keys() and all(current[name].read_bytes() == payload for name, payload in files.items()):
        print(f'Pinned modules {lock["tag"]} already verified')
        return
    temporary = Path(tempfile.mkdtemp(prefix='registry-', dir=ROOT / 'target/module-cache'))
    try:
        for name, payload in files.items():
            path = temporary / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(payload)
        backup = ROOT / 'target/module-cache/previous-registry'
        if not backup.resolve().is_relative_to((ROOT / 'target/module-cache').resolve()):
            raise ValueError('Registry backup resolves outside the module cache')
        if backup.exists():
            shutil.rmtree(backup)
        if registry.exists():
            registry.rename(backup)
        try:
            temporary.rename(registry)
        except OSError:
            if backup.exists():
                backup.rename(registry)
            raise
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)
    print(f'Prepared {len(files)} verified files from {lock["repository"]} {lock["tag"]}')

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, help='use a local copy of the exact pinned snapshot')
    options = parser.parse_args()
    prepare(options.archive)

"""Verify legal material inside the actual portable archives, not just staging."""
import argparse
from pathlib import Path, PurePosixPath
import tarfile
import tempfile
import zipfile

from git_sources import verify_kit
from license_bundle import verify


def check_archive(path):
    if path.name.startswith("Git-") and path.name.endswith("-sources.zip"):
        verify_kit(path)
        return
    with tempfile.TemporaryDirectory(prefix="light-archive-check-", dir=path.parent) as temp:
        root = Path(temp)
        if path.suffix == ".zip":
            with zipfile.ZipFile(path) as archive:
                for name in archive.namelist():
                    relative = PurePosixPath(name.replace("\\", "/"))
                    if relative.is_absolute() or ".." in relative.parts or ":" in name:
                        raise ValueError(f"unsafe archive path: {name}")
                archive.extractall(root)
        else:
            with tarfile.open(path, "r:gz") as archive:
                archive.extractall(root, filter="data")
        payloads = list(root.glob("*/LEGAL_MANIFEST.json"))
        if len(payloads) != 1:
            raise ValueError(f"expected one legal manifest in {path}")
        verify(payloads[0].parent)
    print(f"Portable archive verified: {path.name}", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    paths = sorted([*args.directory.glob("*.zip"), *args.directory.glob("*.tar.gz")])
    if not paths:
        parser.error("no portable archives to verify")
    for path in paths:
        check_archive(path)

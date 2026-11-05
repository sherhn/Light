"""Sign release metadata using an Ed25519 seed supplied only through Actions secrets."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import time


def envelope(payload, seed):
    from nacl.signing import SigningKey
    signed = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    key = SigningKey(bytes.fromhex(seed))
    return {"signed": signed, "signature": key.sign(signed.encode()).signature.hex()}


def artifact(path, base):
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            hasher.update(block)
    return {"name": path.name, "url": base + path.name, "size": path.stat().st_size, "sha256": hasher.hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=["ide", "modules"], required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    parts = args.version.split(".")
    if len(parts) != 3 or not all(p.isascii() and p.isdigit() for p in parts):
        raise SystemExit("Only stable versions may publish automatic updates")
    seed = os.environ.get("LIGHT_UPDATE_SIGNING_KEY", "")
    if len(seed) != 64:
        raise SystemExit("LIGHT_UPDATE_SIGNING_KEY is required; unsigned updates are never published")
    repo = "Light" if args.kind == "ide" else "Light-Modules"
    base = f"https://github.com/sherhn/{repo}/releases/download/v{args.version}/"
    targets = {}
    if args.kind == "ide":
        for path in sorted(args.directory.glob(f"Light-{args.version}-*-update.zip")):
            platform = path.name.removeprefix(f"Light-{args.version}-").removesuffix("-update.zip")
            targets[platform] = artifact(path, base)
        if not {"windows-x64", "linux-x64", "macos-x64", "macos-arm64"} <= set(targets):
            raise SystemExit("All four verified platform update archives are required")
        for pattern, target in [("*-Setup.exe", "windows-x64-installer"), ("*.deb", "linux-x64-installer")]:
            path = next(args.directory.glob(pattern), None)
            if path:
                targets[target] = artifact(path, base)
    else:
        targets["catalog"] = artifact(args.directory / "index.json", base)
    clock = int(time.time())
    payload = {"schema": 1, "kind": args.kind, "version": args.version, "issued": clock, "expires": clock + 366 * 24 * 3600, "targets": targets}
    result = envelope(payload, seed)
    (args.directory / "updates.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"Signed {args.kind} {args.version}: {len(targets)} target(s)")


if __name__ == "__main__":
    main()

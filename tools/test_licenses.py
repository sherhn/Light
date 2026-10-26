import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile
from unittest.mock import patch

import license_bundle as legal
from build_registry import module_notices
import git_sources
from git_sources import inspect_source, versions
from legal_sources import require_sbcl_version


class LicensePolicyTests(unittest.TestCase):
    def test_font_and_unicode_obligations_survive_alternative_selection(self):
        self.assertEqual(legal.choose_license("(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0"), ["MIT", "OFL-1.1", "Ubuntu-font-1.0"])
        self.assertEqual(legal.choose_license("(Apache-2.0 OR MIT) AND Unicode-3.0"), ["MIT", "Unicode-3.0"])
        self.assertEqual(legal.choose_license("Apache-2.0 OR GPL-2.0-only"), ["Apache-2.0"])
        with self.assertRaises(ValueError):
            legal.choose_license("MIT AND GPL-3.0-only")
        with self.assertRaises(ValueError):
            legal.choose_license("(MIT OR Apache-2.0")

    def test_catalogue_detects_lock_changes_and_removed_or_modified_notices(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            text = (legal.ROOT / "LICENSE").read_bytes()
            (root / "NOTICE.txt").write_bytes(text)
            row = {"name": "fixture", "version": "1.0", "expression": "MIT", "selected": ["MIT"], "files": [{"path": "NOTICE.txt", "sha256": legal.digest(text)}]}
            legal.json_file(root / "index.json", {"version": 1, "packages": [row]})
            with patch.object(legal, "CATALOGUE", root):
                legal.validate_catalogue([{"name": "fixture", "version": "1.0", "license": "MIT"}])
                with self.assertRaisesRegex(ValueError, "stale"):
                    legal.validate_catalogue([{"name": "fixture", "version": "2.0", "license": "MIT"}])
                (root / "NOTICE.txt").write_text("MIT", encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "checksum"):
                    legal.validate_catalogue([{"name": "fixture", "version": "1.0", "license": "MIT"}])
                (root / "NOTICE.txt").unlink()
                with self.assertRaises(FileNotFoundError):
                    legal.validate_catalogue([{"name": "fixture", "version": "1.0", "license": "MIT"}])

    def test_manifest_rejects_parent_directory_paths(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            legal.json_file(root / "LEGAL_MANIFEST.json", {"files": [{"path": "../outside", "sha256": "0" * 64}]})
            with self.assertRaises(ValueError):
                legal.verify(root)
            for path in ("..\\outside", "C:/outside", "/outside"):
                with self.assertRaises(ValueError):
                    legal.relative_path(path)

    def test_html_download_carries_original_credit_license_and_change_notice(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            module_notices({"id": "html"}, root)
            notice = (root / "THIRD_PARTY_NOTICES.md").read_text(encoding="utf-8")
            self.assertNotIn(b"\r", (root / "THIRD_PARTY_NOTICES.md").read_bytes())
            self.assertIn("HTML5 Logo by W3C", notice)
            self.assertIn("color, opacity", notice)
            self.assertTrue((root / "HTML5-CC-BY-3.0.txt").is_file())
            self.assertEqual((root / "LICENSE").read_bytes(), (legal.ROOT / "LICENSE").read_bytes())

    def test_unreviewed_runtime_cannot_be_silently_relabelled(self):
        with self.assertRaisesRegex(ValueError, "has not been reviewed"):
            require_sbcl_version("SBCL 99.0.0")


class CorrespondingSourceTests(unittest.TestCase):
    def test_source_kit_checks_every_archive_and_the_reviewed_manifest(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            data = b"exact source bytes"
            lock = {"packages": [{"name": "example", "version": "1", "archive": "example.tar.gz", "sha256": legal.digest(data)}]}
            legal.json_file(root / "lock.json", lock)
            def write(payload, instructions=True):
                with zipfile.ZipFile(root / "kit.zip", "w") as archive:
                    archive.writestr("PACKAGE_SOURCES.json", json.dumps(lock))
                    if instructions:
                        archive.writestr("SOURCE_CODE.md", "original build instructions")
                    archive.writestr("packages/example.tar.gz", payload)
            with patch.object(git_sources, "LOCK", root / "lock.json"):
                write(data)
                git_sources.verify_kit(root / "kit.zip")
                write(b"different source bytes")
                with self.assertRaisesRegex(ValueError, "changed"):
                    git_sources.verify_kit(root / "kit.zip")
                write(data, instructions=False)
                with self.assertRaisesRegex(ValueError, "instructions"):
                    git_sources.verify_kit(root / "kit.zip")

    def source(self, path, version="1.2", upstream=True):
        with tarfile.open(path, "w:gz") as archive:
            files = {"example/PKGBUILD": "# build recipe", "example/.SRCINFO": f"pkgbase = example\n\tpkgver = {version}\n\tpkgrel = 3\n\tlicense = GPL-2.0-only\n\tsource = https://example.org/original.tar.gz\n"}
            if upstream:
                files["example/original.tar.gz"] = "fixture source archive"
            for name, text in files.items():
                data = text.encode()
                info = tarfile.TarInfo(name)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))

    def test_source_package_must_match_version_and_include_upstream_materials(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "example.src.tar.gz"
            self.source(path)
            self.assertEqual(inspect_source(path, "example", "1.2-3")["licenses"], ["GPL-2.0-only"])
            with self.assertRaisesRegex(ValueError, "version"):
                inspect_source(path, "example", "1.3-3")
            self.source(path, upstream=False)
            with self.assertRaises(ValueError):
                inspect_source(path, "example", "1.2-3")

    def test_conflicting_binary_package_versions_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "etc").mkdir()
            (root / "etc/package-versions.txt").write_text("bash 1.0-1\nbash 2.0-1\n")
            with self.assertRaises(ValueError):
                versions(root)


if __name__ == "__main__":
    unittest.main()

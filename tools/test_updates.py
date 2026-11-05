import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

import update_archives


class UpdateArchiveTests(unittest.TestCase):
    def test_reproducible_archives_include_every_owned_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "payload"
            root.mkdir()
            (root / "light.exe").write_bytes(b"program")
            (root / "licenses").mkdir()
            (root / "licenses/NOTICE").write_text("original notices")
            first, second = Path(directory) / "first.zip", Path(directory) / "second.zip"
            update_archives.package(root, first, "1.3.0", "light.exe")
            update_archives.package(root, second, "1.3.0", "light.exe")
            self.assertEqual(first.read_bytes(), second.read_bytes())
            update_archives.verify(first)
            with zipfile.ZipFile(first) as archive:
                manifest = json.loads(archive.read("update-manifest.json"))
                self.assertEqual(set(manifest["files"]), {"light.exe", "licenses/NOTICE"})
                self.assertEqual(manifest["files"]["light.exe"], hashlib.sha256(b"program").hexdigest())

    def test_mac_signature_material_is_allowed_but_extra_files_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "Light.app"
            binary = root / "Contents/MacOS/light"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"program")
            update_archives.inventory(root, "1.3.0", "Contents/MacOS/light", "Contents/Resources/light-update-manifest.json")
            signature = root / "Contents/_CodeSignature/CodeResources"
            signature.parent.mkdir()
            signature.write_text("generated after inventory")
            output = Path(directory) / "update.zip"
            update_archives.package(root, output, "1.3.0", "Contents/MacOS/light", refresh=False)
            update_archives.verify(output)
            with zipfile.ZipFile(output, "a") as archive:
                archive.writestr("unexpected.sh", "bad")
            with self.assertRaises(ValueError):
                update_archives.verify(output)

    def test_zip_traversal_and_payload_corruption_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bad.zip"
            with zipfile.ZipFile(path, "w") as archive:
                archive.writestr("../outside", "bad")
            with self.assertRaises(ValueError):
                update_archives.verify(path)


if __name__ == "__main__":
    unittest.main()

import hashlib
import json
from pathlib import Path
import tempfile
import struct
import unittest
import zipfile

import update_archives


class UpdateArchiveTests(unittest.TestCase):
    @staticmethod
    def macho(signature_size=128):
        header = struct.pack('<8I', 0xFEEDFACF, 0x0100000C, 0, 2, 2, 88, 0, 0)
        segment = struct.pack('<II16sQQQQIIII', 0x19, 72, b'__LINKEDIT', 0, signature_size+256, 120, signature_size+136, 7, 1, 0, 0)
        command = struct.pack('<4I', 0x1D, 16, 256, signature_size)
        return header+segment+command+bytes(range(136))+bytes([42])*signature_size

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

    def test_mac_inventory_survives_resource_resigning_but_detects_changed_code(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "Light.app"
            binary = root / "Contents/MacOS/light"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(self.macho())
            signature = root / "Contents/_CodeSignature/CodeResources"
            signature.parent.mkdir()
            signature.write_text("first signature seal")
            value = update_archives.inventory(root, "1.3.0", "Contents/MacOS/light", "Contents/Resources/light-update-manifest.json")
            self.assertNotIn("Contents/_CodeSignature/CodeResources", value["files"])
            binary.write_bytes(self.macho(256))
            signature.write_text("final signature seals the inventory")
            self.assertEqual(value['files']['Contents/MacOS/light'], 'macho-sha256:'+update_archives.macho_digest(binary))
            output = Path(directory) / "update.zip"
            update_archives.package(root, output, "1.3.0", "Contents/MacOS/light", refresh=False)
            update_archives.verify(output)
            with zipfile.ZipFile(output, "a") as archive:
                archive.writestr("unexpected.sh", "bad")
            with self.assertRaises(ValueError):
                update_archives.verify(output)
            changed = bytearray(binary.read_bytes());changed[200] ^= 1;binary.write_bytes(changed)
            self.assertNotEqual(value['files']['Contents/MacOS/light'], 'macho-sha256:'+update_archives.macho_digest(binary))

    def test_macho_ranges_and_unsigned_binaries_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'light'
            for content in [b'bad', self.macho()[:-1], self.macho()+b'bad']:
                path.write_bytes(content)
                with self.assertRaises(ValueError):update_archives.macho_digest(path)

    def test_zip_traversal_and_payload_corruption_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bad.zip"
            with zipfile.ZipFile(path, "w") as archive:
                archive.writestr("../outside", "bad")
            with self.assertRaises(ValueError):
                update_archives.verify(path)


if __name__ == "__main__":
    unittest.main()

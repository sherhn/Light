import tempfile
from pathlib import Path
import unittest

from installers import desktop_entry, reset_directory, uninstall_manifest
from windows_manifest import APPLICATION, SETTINGS, utf8_manifest
import xml.etree.ElementTree as ET


class InstallerSafetyTests(unittest.TestCase):
    def test_utf8_runtime_manifest_keeps_existing_privileges_and_compatibility(self):
        original = b'''<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
          <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges>
            <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
          </requestedPrivileges></security></trustInfo>
          <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application/></compatibility>
        </assembly>'''
        result = utf8_manifest(original)
        root = ET.fromstring(result)
        self.assertEqual(root.find(f'.//{{{APPLICATION}}}requestedExecutionLevel').attrib['level'], 'asInvoker')
        self.assertIsNotNone(root.find('{urn:schemas-microsoft-com:compatibility.v1}compatibility'))
        setting = f'{{{APPLICATION}}}application/{{{APPLICATION}}}windowsSettings/{{{SETTINGS}}}activeCodePage'
        self.assertEqual(root.find(setting).text, 'UTF-8')
        self.assertEqual(len(ET.fromstring(utf8_manifest(result)).findall(f'.//{{{SETTINGS}}}activeCodePage')), 1)
        self.assertEqual(ET.fromstring(utf8_manifest(None)).find(setting).text, 'UTF-8')

    def test_staging_cleanup_cannot_remove_its_parent_or_an_outside_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            allowed = root / "staging"
            allowed.mkdir()
            protected = root / "protected.txt"
            protected.write_text("keep")
            for path in (allowed, root, root / "outside", allowed / "../outside"):
                with self.assertRaises(ValueError):
                    reset_directory(path, allowed)
            self.assertEqual(protected.read_text(), "keep")
            child = allowed / "payload"
            child.mkdir()
            (child / "old").write_text("old")
            reset_directory(child, allowed)
            self.assertEqual(list(child.iterdir()), [])

    def test_uninstall_targets_only_shipped_files_with_no_recursive_deletion(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "registry").mkdir()
            (root / "light.exe").write_bytes(b"app")
            (root / "registry/module$1.json").write_text("{}")
            commands = uninstall_manifest(root).splitlines()
            self.assertIn('Delete "$INSTDIR\\light.exe"', commands)
            self.assertIn('Delete "$INSTDIR\\registry\\module$$1.json"', commands)
            self.assertIn('RMDir "$INSTDIR\\registry"', commands)
            self.assertFalse(any("/r" in c.lower() or "*" in c or "$APPDATA" in c for c in commands))
            self.assertGreater(commands.index('RMDir "$INSTDIR\\registry"'), commands.index('Delete "$INSTDIR\\registry\\module$$1.json"'))

    def test_desktop_launcher_opens_files_without_a_shell(self):
        desktop = desktop_entry()
        self.assertIn("Exec=/opt/light/light %F\n", desktop)
        self.assertIn("Terminal=false\n", desktop)
        self.assertNotIn("sh -c", desktop)


if __name__ == "__main__":
    unittest.main()

import json
import unittest
import zipfile
import io

import prepare_modules as modules

class PinnedModulesTests(unittest.TestCase):
    def test_pinned_snapshot_is_complete_and_tampering_is_rejected(self):
        lock = json.loads((modules.ROOT / 'modules.lock.json').read_text())
        data = (modules.ROOT / 'target/module-cache' / (lock['sha256'] + '.zip')).read_bytes()
        files = modules.validate(data, lock['sha256'])
        self.assertIn('modules/lisp/module.json', files)
        self.assertIn('modules/html/HTML5-CC-BY-3.0.txt', files)
        with self.assertRaisesRegex(ValueError, 'checksum'):
            modules.validate(data + b'changed', lock['sha256'])

    def test_snapshot_with_an_escaping_entry_is_rejected_even_if_its_hash_matches(self):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as output:
            output.writestr('registry/../../outside', b'no')
        data = stream.getvalue()
        with self.assertRaisesRegex(ValueError, 'Unsafe'):
            modules.validate(data, modules.digest(data))

if __name__ == '__main__':
    unittest.main()

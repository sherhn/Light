import struct
import unittest
from pathlib import Path
import file_types


class FileTypeTests(unittest.TestCase):
    def test_catalog_covers_the_actual_pinned_language_extensions(self):
        self.assertEqual(file_types.load(),file_types.generate())
        mapping=file_types.extensions()
        for extension in ['py','pyi','pyx','cpp','hxx','c++','lisp','scm','jsx','jsonc','http','env','bash','zsh','rst','dockerfile']:
            self.assertIn(extension,mapping)
        self.assertEqual(mapping['dockerfile']['id'],'docker')

    def test_every_family_has_a_multisize_native_icon(self):
        for kind in file_types.load():
            path=file_types.ROOT/'assets/file-icons'/(kind['id']+'.ico')
            data=path.read_bytes()
            reserved,format,count=struct.unpack_from('<HHH',data)
            self.assertEqual((reserved,format,count),(0,1,7))
            dimensions=[]
            for index in range(count):
                offset=6+index*16;size,start=struct.unpack_from('<II',data,offset+8)
                self.assertEqual(data[start:start+8],b'\x89PNG\r\n\x1a\n')
                self.assertLessEqual(start+size,len(data))
                dimensions.append(data[offset] or 256)
            self.assertEqual(dimensions,[16,24,32,48,64,128,256])

    def test_native_registration_has_matching_cleanup_and_no_default_takeover(self):
        script=file_types.nsis_macros('Software\\Isolated-Test')
        self.assertNotIn('UserChoice','\n'.join(script.splitlines()[1:]))
        self.assertNotIn('HKCU "Software\\Classes',script)
        self.assertIn('Light.Source.python\\DefaultIcon',script)
        for extension,kind in file_types.extensions().items():
            self.assertIn(f'DeleteRegValue HKCU "Software\\Isolated-Test\\Classes\\.{extension}\\OpenWithProgids" "Light.Source.{kind["id"]}"',script)


if __name__=='__main__':unittest.main()

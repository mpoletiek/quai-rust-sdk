"""Reject malformed source archives before extracting any package content."""
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1] / 'package_rehearsal.py'
SPEC = importlib.util.spec_from_file_location('package_rehearsal', SOURCE)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PackageArchiveTests(unittest.TestCase):
    def make_archive(self, directory, name, kind=tarfile.REGTYPE):
        path = directory / 'source.crate'
        with tarfile.open(path, 'w:gz') as archive:
            member = tarfile.TarInfo(name)
            member.type = kind
            if kind == tarfile.REGTYPE:
                member.size = 3
                archive.addfile(member, io.BytesIO(b'abc'))
            else:
                member.linkname = '../outside'
                archive.addfile(member)
        return path

    def test_only_regular_files_in_the_named_package_are_extracted(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            archive = self.make_archive(directory, 'quai-test-1/src/lib.rs')
            MODULE.extract(archive, directory / 'out', 'quai-test-1')
            self.assertEqual((directory / 'out/quai-test-1/src/lib.rs').read_bytes(), b'abc')

    def test_traversal_other_packages_and_links_fail_before_writes(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name, kind in [('../escape', tarfile.REGTYPE),
                               ('/absolute', tarfile.REGTYPE),
                               ('other-1/src/lib.rs', tarfile.REGTYPE),
                               ('quai-test-1/../escape', tarfile.REGTYPE),
                               ('quai-test-1/back\\slash', tarfile.REGTYPE),
                               ('quai-test-1/link', tarfile.SYMTYPE),
                               ('quai-test-1/hardlink', tarfile.LNKTYPE)]:
                with self.subTest(name=name):
                    archive = self.make_archive(directory, name, kind)
                    with self.assertRaises(ValueError):
                        MODULE.extract(archive, directory / 'out', 'quai-test-1')
                    self.assertFalse((directory / 'out').exists())


if __name__ == '__main__':
    unittest.main()

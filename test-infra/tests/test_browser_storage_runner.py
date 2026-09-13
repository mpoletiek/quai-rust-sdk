"""Regress the observed Chrome profile cleanup race without depending on timing."""
import errno
import importlib.util
import pathlib
import shutil
import tempfile
import unittest
from unittest import mock

path = pathlib.Path(__file__).resolve().parents[2] / 'crates/quai-browser/tests/run_storage.py'
spec = importlib.util.spec_from_file_location('quai_storage_runner', path)
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class BrowserProfileCleanup(unittest.TestCase):
    def test_child_disappearance_or_late_write_retries_and_removes_owned_profile(self):
        remove = shutil.rmtree
        for error in (errno.ENOENT, errno.ENOTEMPTY):
            with self.subTest(error=error), tempfile.TemporaryDirectory() as root:
                profile = pathlib.Path(root) / 'profile'
                profile.mkdir()
                attempts = []

                def race(path):
                    attempts.append(path)
                    if len(attempts) == 1:
                        raise OSError(error, 'simulated child file race')
                    remove(path)

                with mock.patch.object(runner.shutil, 'rmtree', side_effect=race), mock.patch.object(runner.time, 'sleep'):
                    runner.remove_profile(profile)
                self.assertEqual(len(attempts), 2)
                self.assertFalse(profile.exists())

    def test_missing_root_is_finished_but_permission_failure_is_not_suppressed(self):
        with tempfile.TemporaryDirectory() as root:
            runner.remove_profile(pathlib.Path(root) / 'already-gone')
            with mock.patch.object(runner.shutil, 'rmtree', side_effect=PermissionError(errno.EACCES, 'denied')) as remove:
                with self.assertRaises(PermissionError):
                    runner.remove_profile(root)
                self.assertEqual(remove.call_count, 1)

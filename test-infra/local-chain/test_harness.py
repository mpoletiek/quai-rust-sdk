"""Safety and reproducibility checks for the disposable harness without starting nodes."""
import importlib.util
import pathlib
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('harness',pathlib.Path(__file__).with_name('harness.py'))
harness=importlib.util.module_from_spec(spec);spec.loader.exec_module(harness)

class HarnessTests(unittest.TestCase):
    def test_requires_marker_and_tmp_work_directory(self):
        with tempfile.TemporaryDirectory(prefix='quai-harness-test-') as value:
            work=pathlib.Path(value)
            with self.assertRaises(ValueError):harness.owned(work)
            (work/harness.MARKER).write_text(harness.COMMIT+'\n');harness.owned(work)
        with self.assertRaises(ValueError):harness.owned(pathlib.Path('/'))
    def test_launch_flags_disable_external_network_and_pin_direct_ports(self):
        args=harness.args_for(pathlib.Path('/tmp/public-fixture'))
        for flag in ['--node.solo','--node.portmap=false','--node.telemetry=false','--node.index-address-utxos','--txpool.sync-tx-with-return=false']:self.assertIn(flag,args)
        for flag,value in [('--node.environment','local'),('--node.ipaddr','127.0.0.1'),('--rpc.http-addr','127.0.0.1'),('--rpc.ws-addr','127.0.0.1'),('--rpc.http-port','19001'),('--rpc.ws-port','18001')]:self.assertEqual(args[args.index(flag)+1],value)
    def test_snapshot_digest_detects_mutation_and_rejects_links(self):
        with tempfile.TemporaryDirectory(prefix='quai-harness-test-') as value:
            root=pathlib.Path(value);file=root/'public';file.write_bytes(b'public fixture')
            inventory=harness.inventory_tree(root);file.write_bytes(b'modified')
            self.assertNotEqual(inventory,harness.inventory_tree(root))
            (root/'link').symlink_to(file)
            with self.assertRaises(ValueError):harness.inventory_tree(root)

if __name__=='__main__':unittest.main()

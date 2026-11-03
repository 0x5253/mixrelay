import contextlib
import importlib.util
import io
from pathlib import Path
import shutil
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('signal_trace', Path(__file__).with_name('guest-signal-trace.py'))
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)


class SignalTraceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name) / 'tracefs'
        self.state = Path(self.directory.name) / 'owner.json'
        (self.root / 'events' / 'signal' / 'signal_generate').mkdir(parents=True)
        (self.root / 'instances').mkdir()
        (self.root / 'tracing_on').write_text('existing global trace')
        self.calls = []

    def create(self, path):
        path.mkdir()
        event = path / 'events' / 'signal' / 'signal_generate'
        event.mkdir(parents=True)
        (event / 'format').write_text('sig, code, target pid')
        (path / 'trace').write_text('sender-42: signal_generate: sig=9 comm=driver pid=73')
        cpu = path / 'per_cpu' / 'cpu0'
        cpu.mkdir(parents=True)
        (cpu / 'stats').write_text('overrun: 0')

    def test_private_trace_preserves_sender_target_and_overrun_without_touching_global_trace(self):
        trace.start_trace(self.root, self.state, create=self.create)
        instance = self.root / 'instances' / trace.INSTANCE
        event = instance / 'events' / 'signal' / 'signal_generate'
        self.assertEqual((event / 'filter').read_text(), 'sig == 9 || sig == 24\n')
        with contextlib.redirect_stdout(io.StringIO()) as output:
            trace.stop_trace(self.root, self.state, remove=shutil.rmtree)
        self.assertIn('sender-42', output.getvalue())
        self.assertIn('pid=73', output.getvalue())
        self.assertIn('overrun: 0', output.getvalue())
        self.assertFalse(instance.exists())
        self.assertFalse(self.state.exists())
        self.assertEqual((self.root / 'tracing_on').read_text(), 'existing global trace')

    def test_setup_failure_releases_only_its_own_instance_and_mount(self):
        shutil.rmtree(self.root / 'events')

        def broken(path):
            self.create(path)
            (path / 'buffer_size_kb').mkdir()

        with self.assertRaises(IsADirectoryError):
            trace.start_trace(self.root, self.state,
                              run=lambda args, **kwargs: self.calls.append(args),
                              create=broken, remove=shutil.rmtree)
        self.assertEqual(self.calls, [['mount', '-t', 'tracefs', 'tracefs', str(self.root)],
                                     ['umount', str(self.root)]])
        self.assertFalse(self.state.exists())
        self.assertFalse((self.root / 'instances' / trace.INSTANCE).exists())

    def test_failed_evidence_read_still_releases_the_owned_instance(self):
        trace.start_trace(self.root, self.state, create=self.create)
        instance = self.root / 'instances' / trace.INSTANCE
        (instance / 'trace').unlink()
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(FileNotFoundError):
            trace.stop_trace(self.root, self.state, remove=shutil.rmtree)
        self.assertFalse(instance.exists())
        self.assertFalse(self.state.exists())
        self.assertEqual((self.root / 'tracing_on').read_text(), 'existing global trace')


if __name__ == '__main__':
    unittest.main()

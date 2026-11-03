import importlib.util
from pathlib import Path
import signal
import subprocess
import unittest
from unittest.mock import Mock

spec = importlib.util.spec_from_file_location('linux_stack', Path(__file__).with_name('linux-stack.py'))
stack = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stack)


class StackTest(unittest.TestCase):
    def test_hung_debugger_is_killed_and_reaped_before_resuming_its_tracee(self):
        events = []
        debugger = Mock(pid=77, returncode=-9)
        def wait(timeout):
            events.append(('wait', timeout))
            if timeout == 8 or len(events) == 3:
                raise subprocess.TimeoutExpired('gdb', timeout)
        debugger.wait.side_effect = wait
        result = stack.collect(42, inspect=Mock(side_effect=[('100', 'S', 0), ('100', 'T', 0)]),
                               launch=Mock(return_value=debugger),
                               kill_group=lambda pid, sig: events.append(('killpg', pid, sig)),
                               send=lambda pid, sig: events.append(('send', pid, sig)))
        self.assertEqual(result, 124)
        self.assertEqual(events, [('wait', 8), ('killpg', 77, signal.SIGINT), ('wait', 2),
                                  ('killpg', 77, signal.SIGKILL), ('wait', 2),
                                  ('send', 42, signal.SIGCONT)])

    def test_success_and_pid_reuse_or_another_tracer_never_send_continue(self):
        for after in [None, ('101', 'T', 0), ('100', 't', 88), ('100', 'R', 0)]:
            send = Mock()
            debugger = Mock(pid=77, returncode=0)
            self.assertEqual(stack.collect(42, inspect=Mock(side_effect=[('100', 'S', 0), after]),
                                           launch=Mock(return_value=debugger), send=send), 0)
            send.assert_not_called()

    def test_preexisting_stop_or_debugger_is_preserved(self):
        for before in [('100', 'T', 0), ('100', 't', 88)]:
            launch = Mock()
            with self.assertRaisesRegex(RuntimeError, 'already stopped or traced'):
                stack.collect(42, inspect=lambda pid: before, launch=launch)
            launch.assert_not_called()


if __name__ == '__main__':
    unittest.main()

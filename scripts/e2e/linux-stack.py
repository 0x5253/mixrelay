"""Collect a bounded GDB trace without leaving the installed process stopped."""
from pathlib import Path
import os
import signal
import subprocess
import sys


def identity(pid, proc=Path('/proc')):
    process = proc / str(pid)
    try:
        fields = (process / 'stat').read_text().rsplit(')', 1)[1].split()
        tracer = next(line.split()[1] for line in (process / 'status').read_text().splitlines()
                      if line.startswith('TracerPid:'))
        return fields[19], fields[0], int(tracer)
    except FileNotFoundError:
        return None


def collect(pid, *, inspect=identity, launch=subprocess.Popen, kill_group=os.killpg, send=os.kill):
    before = inspect(pid)
    if before is None:
        return 0
    if before[1] in ('T', 't') or before[2] != 0:
        raise RuntimeError('Refusing to attach to an already stopped or traced process')
    debugger = launch(['gdb', '--batch', '--nx', '-p', str(pid), '-ex', 'set pagination off',
                       '-ex', 'thread apply all bt', '-ex', 'detach'], start_new_session=True)
    timed_out = False
    try:
        try:
            debugger.wait(timeout=8)
        except subprocess.TimeoutExpired:
            timed_out = True
            kill_group(debugger.pid, signal.SIGINT)
            try:
                debugger.wait(timeout=2)
            except subprocess.TimeoutExpired:
                kill_group(debugger.pid, signal.SIGKILL)
                debugger.wait(timeout=2)
    finally:
        after = inspect(pid)
        # Resume only the same, initially running process after our tracer has exited.
        if after and after[0] == before[0] and after[1] in ('T', 't') and after[2] == 0:
            send(pid, signal.SIGCONT)
    if timed_out:
        print('GDB collection timed out; debugger reaped and target released', file=sys.stderr)
        return 124
    return debugger.returncode


if __name__ == '__main__':
    sys.exit(collect(int(sys.argv[1])))

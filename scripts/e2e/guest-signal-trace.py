#!/usr/bin/env python3
"""Record SIGKILL and SIGXCPU senders inside the disposable qualification VM."""

import argparse
import json
from pathlib import Path
import subprocess


TRACEFS = Path('/sys/kernel/tracing')
STATE = Path('/run/mixrelay-qualification-trace.json')
INSTANCE = 'mixrelay-qualification'


def start_trace(root=TRACEFS, state=STATE, run=subprocess.run,
                create=lambda path: path.mkdir(), remove=lambda path: path.rmdir()):
    if state.exists():
        raise RuntimeError('Qualification signal trace already exists')
    mounted = False
    instance = root / 'instances' / INSTANCE
    owned = False
    try:
        if not (root / 'events' / 'signal' / 'signal_generate').is_dir():
            run(['mount', '-t', 'tracefs', 'tracefs', str(root)], check=True)
            mounted = True
        create(instance)
        owned = True
        (instance / 'tracing_on').write_text('0\n')
        (instance / 'buffer_size_kb').write_text('256\n')
        event = instance / 'events' / 'signal' / 'signal_generate'
        (event / 'filter').write_text('sig == 9 || sig == 24\n')
        (event / 'enable').write_text('1\n')
        (instance / 'tracing_on').write_text('1\n')
        with state.open('x') as output:
            json.dump({'mountedTracefs': mounted}, output)
    except BaseException:
        if owned:
            (instance / 'tracing_on').write_text('0\n')
            remove(instance)
        if mounted:
            run(['umount', str(root)], check=True)
        raise


def stop_trace(root=TRACEFS, state=STATE, run=subprocess.run,
               remove=lambda path: path.rmdir()):
    ownership = json.loads(state.read_text())
    instance = root / 'instances' / INSTANCE
    try:
        (instance / 'tracing_on').write_text('0\n')
        (instance / 'events' / 'signal' / 'signal_generate' / 'enable').write_text('0\n')
        print('Qualification signal_generate format:')
        print((instance / 'events' / 'signal' / 'signal_generate' / 'format').read_text())
        print('Qualification signal trace (sender context precedes target fields):')
        print((instance / 'trace').read_text())
        for stats in sorted((instance / 'per_cpu').glob('cpu*/stats')):
            print(f'{stats.relative_to(instance)}:\n{stats.read_text()}')
    finally:
        remove(instance)
        state.unlink()
        if ownership['mountedTracefs']:
            run(['umount', str(root)], check=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['start', 'stop'])
    parser.add_argument('--disposable-environment', action='store_true', required=True)
    args = parser.parse_args()
    (start_trace if args.action == 'start' else stop_trace)()

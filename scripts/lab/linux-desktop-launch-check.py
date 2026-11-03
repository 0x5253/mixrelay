#!/usr/bin/env python3
"""Launch the installed desktop entry from an inactive user service in an owned Xvfb.

Usage: python3 scripts/lab/linux-desktop-launch-check.py /absolute/new/evidence
Requires a normal Linux user, installed MixRelay, Xvfb, curl and Python Gdk 3.
Refuses existing app/engine owners; restores the initially inactive Driver unit.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


APP = Path('/usr/bin/mixrelay-desktop')
ENGINE = Path('/usr/bin/mixrelay-service')
UNIT = 'mixrelay-driver.service'


def owners(executable):
    found = {}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            if entry.stat().st_uid == os.getuid() and entry.joinpath('exe').resolve() == executable:
                found[int(entry.name)] = entry.joinpath('stat').read_text().rsplit(')', 1)[1].split()[19]
        except (OSError, ValueError):
            continue
    return found


def unit_pid():
    return subprocess.check_output(['systemctl', '--user', 'show', UNIT, '--property=MainPID', '--value'], text=True).strip()


def until(check, seconds=20):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        result = check()
        if result:
            return result
        time.sleep(0.1)
    raise RuntimeError('Installed desktop readiness timed out')


def api(role, route):
    result = subprocess.run(['curl', '--unix-socket', str(Path(os.environ['XDG_RUNTIME_DIR']) / 'mixrelay' / f'{role}.sock'),
                             '--silent', '--fail', '--max-time', '1', '-H', f'Host: mixrelay-{role}',
                             f'http://mixrelay-{role}{route}'], capture_output=True, text=True, check=False)
    if result.returncode:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return {'reachable': True, 'body': result.stdout}


def main():
    if sys.platform != 'linux' or os.getuid() == 0 or len(sys.argv) != 2:
        raise RuntimeError(__doc__)
    evidence = Path(sys.argv[1])
    assert evidence.is_absolute() and APP.is_file() and ENGINE.is_file()
    assert not owners(APP) and not owners(ENGINE), 'Refusing existing installed owners'
    assert unit_pid() == '0', 'Driver user unit must initially be inactive'
    evidence.mkdir(mode=0o700)
    os.environ['XDG_RUNTIME_DIR'] = f'/run/user/{os.getuid()}'
    os.environ['DBUS_SESSION_BUS_ADDRESS'] = f"unix:path={os.environ['XDG_RUNTIME_DIR']}/bus"
    env = {**os.environ, 'GDK_BACKEND': 'x11', 'LIBGL_ALWAYS_SOFTWARE': '1', 'WEBKIT_DISABLE_DMABUF_RENDERER': '1'}
    applications = {}
    driver_pid = '0'
    with (evidence / 'display').open('w') as display, (evidence / 'xvfb.log').open('w') as xvfb_log:
        server = subprocess.Popen(['Xvfb', '-displayfd', str(display.fileno()), '-screen', '0', '1440x900x24', '-nolisten', 'tcp'],
                                  pass_fds=(display.fileno(),), stdout=subprocess.DEVNULL, stderr=xvfb_log)
    try:
        number = until(lambda: (evidence / 'display').read_text().strip(), 5)
        env['DISPLAY'] = ':' + number
        os.environ['DISPLAY'] = env['DISPLAY']
        with (evidence / 'application.log').open('w') as log:
            subprocess.run(['gio', 'launch', '/usr/share/applications/MixRelay.desktop'], env=env,
                           stdout=log, stderr=log, check=True)
        applications = until(lambda: owners(APP))
        driver_pid = until(lambda: value if (value := unit_pid()) != '0' else None)
        driver = until(lambda: api('driver', '/api/v1/driver'))
        assert driver['state'] == 'ready' and int(driver_pid) > 0, driver
        health = until(lambda: api('engine', '/health'))
        applications.update(owners(APP))
        for name, value in [('driver.json', driver), ('engine-health.json', health)]:
            (evidence / name).write_text(json.dumps(value, indent=2) + '\n')
        time.sleep(3)
        assert owners(APP), 'Desktop exited before rendering'
        applications.update(owners(APP))
        import gi
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gdk
        Gdk.init([])
        root = Gdk.get_default_root_window()
        image = Gdk.pixbuf_get_from_window(root, 0, 0, root.get_width(), root.get_height())
        assert image is not None
        image.savev(str(evidence / 'desktop.png'), 'png', [], [])
        print('PASS: installed .desktop launch started Driver Ready and the owned engine; rendered screenshot saved', flush=True)
    finally:
        for pid, start in applications.items():
            if owners(APP).get(pid) == start:
                os.kill(pid, signal.SIGTERM)
        try:
            until(lambda: not owners(APP) and not owners(ENGINE), 10)
        finally:
            current = unit_pid()
            try:
                if driver_pid != '0' and current == driver_pid:
                    subprocess.run(['systemctl', '--user', 'stop', UNIT], check=True)
            finally:
                server.terminate()
                server.wait(timeout=5)
                (evidence / 'cleanup.json').write_text(json.dumps({'appOwners': owners(APP), 'engineOwners': owners(ENGINE), 'driverPid': unit_pid()}) + '\n')


if __name__ == '__main__':
    main()

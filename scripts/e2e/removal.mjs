import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { installedPaths, retiredPaths } from './package.mjs';

function execute(program, args) {
  const result = spawnSync(program, args, {
    encoding: 'utf8',
    timeout: 300000,
    maxBuffer: 8 * 1024 * 1024,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, `${program}: ${result.stdout}\n${result.stderr}`);
  return `${result.stdout ?? ''}\n${result.stderr ?? ''}`;
}

export function removeInstalled(os, format, output, run = execute, exists = existsSync) {
  let log = '';
  try {
    if (os === 'macos') log = removeMacos(run);
    else {
      log =
        format === 'deb'
          ? run('sudo', ['-n', 'apt-get', 'remove', '-y', 'mix-relay'])
          : run('sudo', ['-n', 'pacman', '-R', '--noconfirm', 'mix-relay']);
      run('systemctl', ['--user', 'daemon-reload']);
    }
    const owned = [
      ...installedPaths(os),
      ...(os === 'macos'
        ? [
            '/Applications/MixRelay/MixRelay.app',
            '/Applications/MixRelay/Uninstall MixRelay.app',
            '/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver',
            '/Library/LaunchDaemons/io.mixrelay.audio.engine.plist',
            '/Library/LaunchDaemons/io.mixrelay.audio.driver.plist',
            '/Library/LaunchDaemons/io.mixrelay.driver.removal.plist',
          ]
        : ['/usr/lib/systemd/user/mixrelay-driver.service']),
      ...retiredPaths(os),
    ];
    const left = owned.filter((path) => exists(path));
    assert.ok(left.length === 0, `Removal left installed product artifacts: ${left.join(', ')}`);
  } catch (error) {
    log += `\n${error.stack}`;
    throw error;
  } finally {
    writeFileSync(join(output, 'removal.log'), log, { flag: 'wx' });
  }
}

function removeMacos(run) {
  const helper =
    '/Applications/MixRelay/Uninstall MixRelay.app/Contents/Helpers/mixrelay-macos-installer';
  const signature = run('codesign', ['-dv', '--verbose=4', helper]);
  const team = signature.match(/(?:^|\n)TeamIdentifier=([A-Z0-9]{10})(?:\n|$)/)?.[1];
  assert.ok(team, 'Installed removal helper has no publisher identity');
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-e2e-removal-'));
  const copy = join(directory, 'installer');
  try {
    run('ditto', [helper, copy]);
    run('codesign', ['--verify', '--strict', copy]);
    return run('sudo', ['-n', copy, 'remove', team, '--reload-coreaudio']);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

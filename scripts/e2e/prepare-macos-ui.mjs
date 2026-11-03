import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

function run(program, args) {
  const result = spawnSync(program, args, { encoding: 'utf8', env: { ...process.env, HOMEBREW_NO_AUTO_UPDATE: '1' } });
  process.stdout.write(result.stdout ?? '');
  process.stderr.write(result.stderr ?? '');
  return result;
}

export async function prepareMacosUi({ command = run, wait = delay } = {}) {
  if (command('xcodegen', ['--version']).status === 0) return;
  for (let attempt = 1; attempt <= 3; attempt++) {
    const result = command('brew', ['install', 'xcodegen']);
    if (result.status === 0) break;
    const diagnostic = `${result.stdout ?? ''}\n${result.stderr ?? ''}`;
    assert.ok(attempt < 3 && /curl: \(6\) Could not resolve host:/.test(diagnostic),
      `XcodeGen installation failed on attempt ${attempt}`);
    await wait(attempt * 2000);
  }
  const installed = command('xcodegen', ['--version']);
  assert.ifError(installed.error);
  assert.equal(installed.status, 0, 'Installed XcodeGen must execute before UI qualification');
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  assert.equal(process.platform, 'darwin');
  await prepareMacosUi();
}

import assert from 'node:assert/strict';
import { once } from 'node:events';
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { installedPaths, requireDisposableHost, spawnLogged, waitForInstalledExit } from './package.mjs';

const ticketTransport = (_output, verify) => verify();

test('stapler retries a lost Apple connection and requires validation before extraction', async (t) => {
  const { installAndVerify } = await import('./package.mjs');
  const output = mkdtempSync(join(tmpdir(), 'mr-stapler-network-'));
  t.after(() => rmSync(output, { recursive: true, force: true }));
  const calls = [], waits = [];
  let attempts = 0, elapsed = 0;
  const runCommand = (program, args) => {
    calls.push([program, args]);
    if (program === 'xcrun') {
      attempts++;
      if (elapsed < 30000) throw Object.assign(new Error('stapler exited 68'), {
        status: 68, stderr: 'NSURLErrorDomain Code=-1005 "The network connection was lost."',
      });
    }
    if (program === 'pkgutil' && args[0] === '--expand-full') {
      assert.equal(attempts, 2);
      mkdirSync(args[2]);
    }
    return 'accepted';
  };
  installAndVerify({ file: 'candidate.pkg' }, {}, output, output, 'macos', {
    runCommand, ticketTransport, paths: [], wait: (milliseconds) => { waits.push(milliseconds); elapsed += milliseconds; },
  });
  assert.equal(calls.at(-1)[0], 'sudo');
  assert.deepEqual(waits, [30000]);
  const report = JSON.parse(readFileSync(join(output, 'package-verification.json')));
  assert.deepEqual(report.filter((entry) => entry.program === 'xcrun').map((entry) => entry.status), [68, 0]);
  assert.match(report[2].stderr, /Code=-1005/);
});

test('persistent stapler transport failure is bounded and never installs', async (t) => {
  const { installAndVerify } = await import('./package.mjs');
  const output = mkdtempSync(join(tmpdir(), 'mr-stapler-offline-'));
  t.after(() => rmSync(output, { recursive: true, force: true }));
  let attempts = 0;
  const waits = [];
  const runCommand = (program, args) => {
    assert.notEqual(program, 'sudo');
    assert.notEqual(args[0], '--expand-full');
    if (program === 'xcrun') {
      attempts++;
      throw Object.assign(new Error('stapler exited 68'), {
        status: 68, stderr: 'NSURLErrorDomain Code=-1005 "The network connection was lost."',
      });
    }
    return 'accepted';
  };
  assert.throws(() => installAndVerify({ file: 'candidate.pkg' }, {}, output, output, 'macos', {
    runCommand, ticketTransport, wait: (milliseconds) => waits.push(milliseconds),
  }), /stapler exited 68/);
  assert.equal(attempts, 3);
  assert.deepEqual(waits, [30000, 60000]);
  const report = JSON.parse(readFileSync(join(output, 'package-verification.json')));
  assert.equal(report.filter((entry) => entry.program === 'xcrun').length, 3);
});

test('stapler failure preserves raw diagnostics and stops before extraction or installation', async (t) => {
  const { installAndVerify } = await import('./package.mjs');
  const output = mkdtempSync(join(tmpdir(), 'mr-stapler-'));
  t.after(() => rmSync(output, { recursive: true, force: true }));
  const calls = [];
  const runCommand = (program, args) => {
    calls.push([program, args]);
    if (program === 'xcrun') {
      throw Object.assign(new Error('stapler exited 65'), {
        status: 65, stdout: 'The validate action failed!', stderr: 'Ticket signature is invalid',
      });
    }
    return 'accepted';
  };
  assert.throws(() => installAndVerify({ file: 'candidate.pkg' }, {}, output, output, 'macos', {
    runCommand, ticketTransport,
  }), /stapler exited 65\nThe validate action failed!\nTicket signature is invalid/);
  assert.deepEqual(calls.map(([program]) => program), ['pkgutil', 'spctl', 'xcrun']);
  const report = JSON.parse(readFileSync(join(output, 'package-verification.json')));
  assert.equal(report[2].status, 65);
  assert.equal(report[2].stderr, 'Ticket signature is invalid');
  assert.equal(report[2].stdout, 'The validate action failed!');
});

test('installation refuses an unacknowledged host before any installer or privilege command runs', () => {
  assert.throws(() => requireDisposableHost('macos', false), /disposable-environment/);
  assert.throws(() => requireDisposableHost('linux', false), /disposable-environment/);
  assert.ok(installedPaths('macos').every((path) => path.startsWith('/')));
  assert.ok(installedPaths('linux').every((path) => path.startsWith('/usr/bin/')));
});

test('a background controller inheriting stderr cannot keep the completed desktop test open', async (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-desktop-lifetime-'));
  const pidFile = join(directory, 'runtime.pid'),
    logFile = join(directory, 'desktop.log');
  t.after(() => {
    try {
      process.kill(Number(readFileSync(pidFile, 'utf8')), 'SIGTERM');
    } catch (error) {
      if (!['ENOENT', 'ESRCH'].includes(error.code)) throw error;
    }
    rmSync(directory, { recursive: true, force: true });
  });
  const child = spawnLogged(
    process.execPath,
    [
      '-e',
      `
    const {spawn}=require('node:child_process'), {writeFileSync}=require('node:fs');
    const runtime=spawn(process.execPath,['-e','setTimeout(()=>{},5000)'],{stdio:['ignore','ignore','inherit']});
    writeFileSync(process.argv[1],String(runtime.pid));
    runtime.unref(); process.stderr.write('desktop completed\\n');
  `,
      pidFile,
    ],
    logFile,
  );
  const [code] = await once(child, 'close', { signal: AbortSignal.timeout(1500) });
  assert.equal(code, 0);
  assert.match(readFileSync(logFile, 'utf8'), /desktop completed/);
});

test('same-version DEBs are reinstalled and installed executable bytes are checked', async (t) => {
  const { mkdirSync, writeFileSync } = await import('node:fs');
  const { installAndVerify } = await import('./package.mjs');
  const root = mkdtempSync(join(tmpdir(), 'mixrelay-reinstall-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const output = join(root, 'evidence');
  mkdirSync(output);
  const executable = join(root, 'installed-binary');
  writeFileSync(executable, 'candidate payload');
  const calls = [];
  const runCommand = (program, args) => {
    calls.push([program, args]);
    if (program === 'dpkg-deb') {
      const path = join(args[2], executable.slice(1));
      mkdirSync(join(path, '..'), { recursive: true });
      writeFileSync(path, 'candidate payload');
    }
    return 'installer succeeded';
  };
  installAndVerify({ file: 'candidate.deb' }, { format: 'deb' }, root, output, 'linux', {
    runCommand, ticketTransport,
    paths: [executable],
  });
  assert.deepEqual(calls[1], [
    'sudo',
    [
      '-n',
      'env',
      'DEBIAN_FRONTEND=noninteractive',
      'apt-get',
      'install',
      '--reinstall',
      '-y',
      '--no-install-recommends',
      join(root, 'candidate.deb'),
    ],
  ]);
  const report = JSON.parse(readFileSync(join(output, 'installed-binaries.json')));
  assert.equal(report.binaries[0].path, executable);
  const other = join(root, 'mismatch');
  mkdirSync(other);
  writeFileSync(executable, 'baseline payload');
  assert.throws(
    () =>
      installAndVerify({ file: 'candidate.deb' }, { format: 'deb' }, root, other, 'linux', {
        runCommand, ticketTransport,
        paths: [executable],
      }),
    /differs from candidate/,
  );
});

test('a macOS postinstall rejection is fatal and preserves installer stderr', async (t) => {
  const { mkdirSync } = await import('node:fs');
  const { installAndVerify } = await import('./package.mjs');
  const root = mkdtempSync(join(tmpdir(), 'mixrelay-postinstall-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const output = join(root, 'evidence');
  mkdirSync(output);
  const commands = [];
  const runCommand = (program, args) => {
    commands.push([program, args]);
    if (program === 'sudo') {
      const error = new Error('installer exited 1');
      error.stderr = 'Previous app entitlement policy rejected';
      throw error;
    }
    return '';
  };
  assert.throws(
    () =>
      installAndVerify({ file: 'signed.pkg' }, { format: 'pkg' }, root, output, 'macos', {
        runCommand, ticketTransport,
      }),
    /installer exited 1/,
  );
  assert.deepEqual(commands.at(-1), [
    'sudo',
    ['-n', 'installer', '-dumplog', '-pkg', join(root, 'signed.pkg'), '-target', '/'],
  ]);
  assert.match(
    readFileSync(join(output, 'package-install.log'), 'utf8'),
    /entitlement policy rejected/,
  );
});


test('installed shutdown observes asynchronous audio finalization and editor exit on one deadline', async () => {
  let time = 0;
  const result = await waitForInstalledExit(() => ({ runtimes: time < 6000 ? ['8821'] : [],
    editorClosed: time >= 8000 }), { now: () => time, wait: async ms => { time += ms; } });
  assert.equal(result.stopped, true);
  assert.equal(result.elapsedMs, 8000);
  assert.deepEqual(result.runtimes, []);
  assert.equal(result.editorClosed, true);
});

test('a runtime that never finishes fails bounded shutdown observation without a force kill', async () => {
  let time = 0;
  const result = await waitForInstalledExit(() => ({ runtimes: ['8821'], editorClosed: true }),
    { now: () => time, wait: async ms => { time += ms; } });
  assert.equal(result.stopped, false);
  assert.ok(result.elapsedMs <= 150000);
  assert.deepEqual(result.runtimes, ['8821']);
});

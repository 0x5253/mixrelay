import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { existsSync, lstatSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { cleanupCIMacosSigning, withCIMacosSigning } from './prepare-ci-macos-signing.mjs';
import { RETIRED_SIGNING_KEYCHAIN } from './macos-signing.mjs';

function fixture(t) {
  const runner = mkdtempSync(join(tmpdir(), 'mixrelay-ci-signing-test-'));
  t.after(() => rmSync(runner, { recursive: true, force: true }));
  const encoded = value => Buffer.from(value).toString('base64');
  const env = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: '0x5253/mixrelay', RUNNER_TEMP: runner,
    GITHUB_ENV: join(runner, 'github-env'), MIXRELAY_MACOS_TEAM_ID: 'AB12C3D4E5',
    MIXRELAY_MACOS_APPLICATION_IDENTITY: 'Developer ID Application: Fixture (AB12C3D4E5)',
    MIXRELAY_MACOS_INSTALLER_IDENTITY: 'Developer ID Installer: Fixture (AB12C3D4E5)',
    MIXRELAY_MACOS_APPLICATION_P12_BASE64: encoded('application-fixture'),
    MIXRELAY_MACOS_INSTALLER_P12_BASE64: encoded('installer-fixture'),
    MIXRELAY_MACOS_P12_PASSWORD: 'fixture-p12-secret',
    MIXRELAY_NOTARIZATION_KEY_BASE64: encoded('notarization-fixture'),
    MIXRELAY_NOTARIZATION_KEY_ID: 'ABC123DE45', MIXRELAY_NOTARIZATION_ISSUER: '12345678-1234-1234-1234-123456789abc' };
  const calls = [];
  const login = '/Users/runner/Library/Keychains/login.keychain-db';
  const signals = Object.assign(new EventEmitter(), { pid: 123 });
  let fail;
  const exec = (program, args) => {
    assert.equal(program, '/usr/bin/security');
    calls.push(args);
    if (fail?.(args)) return { status: 1, stderr: Object.values(env).join('\n') };
    if (args[0] === 'create-keychain') writeFileSync(args.at(-1), 'fixture-keychain');
    return { status: 0, stdout: args.length === 3 && args[0] === 'list-keychains'
      ? [login, RETIRED_SIGNING_KEYCHAIN].map(path => `    ${JSON.stringify(path)}`).join('\n') : '' };
  };
  return { env, exec, calls, signals, login, runner, platform: 'darwin',
    preflight: () => {},
    sign: (action, path) => action({ ...env }, JSON.parse(readFileSync(path)), () => {}),
    fail: value => { fail = value; } };
}
function restored(f) {
  assert.deepEqual(f.calls.findLast(args => args[0] === 'list-keychains' && args.includes('-s')), ['list-keychains', '-d', 'user', '-s', f.login]);
  assert.ok(f.calls.some(args => args[0] === 'delete-keychain'));
  assert.ok(!readdirSync(f.runner).some(name => name.startsWith('mixrelay-signing-')));
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) assert.equal(f.signals.listenerCount(signal), 0);
}

test('CI assets stay owner-only, authorize only signing tools, preflight before the owned build, and are removed', async t => {
  const f = fixture(t);
  const order = [];
  f.preflight = path => {
    order.push('preflight');
    const directory = join(path, '..');
    assert.equal(lstatSync(directory).mode & 0o777, 0o700);
    for (const file of ['config.json', 'application.p12', 'installer.p12', 'AuthKey.p8', 'search-list.json']) {
      assert.equal(lstatSync(join(directory, file)).mode & 0o777, 0o600);
    }
  };
  const result = await withCIMacosSigning((env, config) => {
    order.push('build');
    assert.ok(existsSync(env.MIXRELAY_SIGNING_CONFIG));
    for (const name of Object.keys(f.env).filter(name => /P12|NOTARIZATION/.test(name))) assert.equal(env[name], undefined);
    assert.equal(config.notarization.keyId, f.env.MIXRELAY_NOTARIZATION_KEY_ID);
    assert.equal(readFileSync(config.notarization.apiKeyPath, 'utf8'), 'notarization-fixture');
    return 42;
  }, f);
  assert.equal(result, 42);
  assert.deepEqual(order, ['preflight', 'build']);
  const imports = f.calls.filter(args => args[0] === 'import');
  assert.equal(imports.length, 2);
  assert.ok(imports.every(args => !args.includes('-A') && args.includes('/usr/bin/codesign') && args.includes('/usr/bin/pkgbuild') && args.includes('/usr/bin/productbuild')));
  assert.ok(f.calls.some(args => args[0] === 'set-key-partition-list' && args.includes('apple-tool:,apple:,codesign:')));
  assert.ok(f.calls.some(args => args[0] === 'unlock-keychain'));
  assert.ok(f.calls.some(args => args[0] === 'set-keychain-settings' && args.includes('300')));
  restored(f);
});

test('invalid environment, identity and secret encodings fail before touching any keychain', async t => {
  const f = fixture(t);
  for (const patch of [
    { GITHUB_REPOSITORY: 'another/repository' }, { GITHUB_ACTIONS: 'false' },
    { MIXRELAY_MACOS_APPLICATION_P12_BASE64: 'not base64!' },
    { MIXRELAY_MACOS_INSTALLER_IDENTITY: 'Developer ID Installer: Wrong (ZZ12C3D4E5)' },
    { MIXRELAY_NOTARIZATION_KEY_ID: 'wrong' }, { MIXRELAY_MACOS_P12_PASSWORD: '' },
  ]) {
    await assert.rejects(withCIMacosSigning(() => assert.fail('no build'), { ...f, env: { ...f.env, ...patch } }));
  }
  assert.deepEqual(f.calls, []);
});

test('failed import, preflight and build each restore search paths and erase every private asset', async t => {
  for (const stage of ['import', 'preflight', 'build']) {
    const f = fixture(t);
    if (stage === 'import') f.fail(args => args[0] === 'import');
    if (stage === 'preflight') f.preflight = () => { throw new Error('preflight failure'); };
    await assert.rejects(withCIMacosSigning(() => { throw new Error('build failure'); }, f), error => {
      assert.ok(!error.message.includes(f.env.MIXRELAY_MACOS_P12_PASSWORD));
      assert.ok(!error.message.includes(f.env.MIXRELAY_MACOS_APPLICATION_P12_BASE64));
      return true;
    });
    restored(f);
  }
});

test('the owned signing wrapper finishes interruption cleanup before CI assets are deleted', async t => {
  const f = fixture(t);
  f.sign = async (_action, path, io) => {
    assert.ok(existsSync(path));
    f.signals.emit('SIGTERM');
    // The existing wrapper stops and awaits its owned build and refresh processes first.
    io.signals.kill(io.signals.pid, 'SIGTERM');
  };
  await assert.rejects(withCIMacosSigning(() => assert.fail('no build'), f), /interrupted by SIGTERM/);
  restored(f);
});

test('the always-run cleanup accepts only this runner\'s private directory and is repeatable', async t => {
  const f = fixture(t);
  const outside = join(f.runner, 'unrelated');
  writeFileSync(outside, 'preserved');
  assert.throws(() => cleanupCIMacosSigning(outside, f), /outside the CI signing directory/);
  const symlink = join(f.runner, 'mixrelay-signing-link');
  symlinkSync(f.runner, symlink);
  assert.throws(() => cleanupCIMacosSigning(symlink, f), /private to its owner/);
  assert.ok(lstatSync(symlink).isSymbolicLink());
  rmSync(symlink);
  await withCIMacosSigning(() => {}, f);
  const directory = readFileSync(f.env.GITHUB_ENV, 'utf8').trim().split('=')[1];
  cleanupCIMacosSigning(directory, f);
  assert.equal(readFileSync(outside, 'utf8'), 'preserved');
  restored(f);
});

test('keychain deletion failure still erases private files and fails the job', async t => {
  const f = fixture(t);
  f.fail(args => args[0] === 'delete-keychain');
  await assert.rejects(withCIMacosSigning(() => {}, f), /delete the temporary build keychain/);
  restored(f);
});


test('preflight and build refresh processes finish before any private configuration is deleted', async t => {
  const f = fixture(t);
  const exited = [];
  f.spawnRefresh = path => {
    const child = Object.assign(new EventEmitter(), { exitCode: null, signalCode: null });
    child.kill = signal => {
      assert.ok(existsSync(path), 'refresh retains its configuration until it exits');
      setImmediate(() => {
        assert.ok(existsSync(path), 'cleanup must wait for the child exit');
        child.signalCode = signal;
        exited.push(path);
        child.emit('exit', null, signal);
        child.emit('close', null, signal);
      });
      return true;
    };
    return child;
  };
  f.preflight = (path, io) => { io.startRefresh(path, f.signals.pid); };
  f.sign = (action, path, io) => {
    io.startRefresh(path, f.signals.pid);
    return action(f.env, JSON.parse(readFileSync(path)), () => {});
  };
  await withCIMacosSigning(() => {}, f);
  assert.equal(exited.length, 2);
  restored(f);
});


test('a failed unlock refresh aborts before building and still removes its private assets', async t => {
  const f = fixture(t);
  f.spawnRefresh = () => {
    const child = Object.assign(new EventEmitter(), { exitCode: null, signalCode: null });
    child.kill = () => true;
    setImmediate(() => {
      child.exitCode = 1;
      child.emit('close', 1, null);
    });
    return child;
  };
  f.preflight = async (path, io) => {
    io.startRefresh(path, f.signals.pid);
    await new Promise(resolve => setImmediate(resolve));
  };
  await assert.rejects(withCIMacosSigning(() => assert.fail('no build after refresh failure'), f), /interrupted by SIGTERM/);
  restored(f);
});

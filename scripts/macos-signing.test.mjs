import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { EventEmitter, once } from 'node:events';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { RETIRED_SIGNING_KEYCHAIN, DEFAULT_SIGNING_CONFIG, SIGNING_BACKUPS, SIGNING_INTERRUPTIONS, pemCertificateSha1, restoreMacosSigningKeychain, usableKeychainSearchList, withMacosSigning } from './macos-signing.mjs';

test('retired release lookup is removed without removing other identities or restored keys', () => {
  const login = '/Library/Keychains/unrelated-login.keychain-db';
  const restored = join(dirname(DEFAULT_SIGNING_CONFIG), 'release.keychain-db');
  const unrelatedRelease = '/another/driver-20261008/release.keychain-db';
  const original = [login, RETIRED_SIGNING_KEYCHAIN, restored, unrelatedRelease];
  assert.deepEqual(usableKeychainSearchList(original), [login, restored, unrelatedRelease]);
  assert.equal(original.length, 4);
});

test('a clean search list stays stable across subsequent builds', () => {
  const original = ['/Library/Keychains/login.keychain-db'];
  assert.deepEqual(usableKeychainSearchList(usableKeychainSearchList(original)), original);
});

// Mocked `security`: no keychain, password or search list of this machine is touched.
function signingFixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-signing-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const keychain = join(directory, 'release.keychain-db');
  writeFileSync(keychain, 'not a keychain');
  const config = join(directory, 'config.json');
  writeFileSync(config, JSON.stringify({
    keychain, teamId: 'AB12C3D4E5', keychainPassword: 'fixture-keychain-secret', p12Password: 'fixture-p12-secret',
    applicationIdentity: 'Developer ID Application: Fixture (AB12C3D4E5)',
    installerIdentity: 'Developer ID Installer: Fixture (AB12C3D4E5)',
  }), { mode: 0o600 });
  const login = '/Users/fixture/Library/Keychains/login.keychain-db';
  return { directory, keychain, config, login, original: [login, RETIRED_SIGNING_KEYCHAIN] };
}
function mockSecurity(fixture, calls) {
  return (program, args) => {
    calls.push([program, ...args]);
    if (args[0] === 'find-identity') return { status: 0, stdout: `1) A "Developer ID Application: Fixture (AB12C3D4E5)"\n2) B "Developer ID Installer: Fixture (AB12C3D4E5)"\n` };
    if (args[0] === 'list-keychains' && args.length === 3) return { status: 0, stdout: fixture.original.map(path => `    "${path}"`).join('\n') };
    return { status: 0, stdout: '' };
  };
}
/** A stand-in process that exits on the next turn after any signal. */
function fakeProcess() {
  const process = new EventEmitter();
  process.killed = [];
  process.kill = signal => { process.killed.push(signal); setImmediate(() => process.emit('exit', null, signal)); };
  return process;
}
const searchListWrites = calls => calls.filter(call => call[1] === 'list-keychains' && call.includes('-s')).map(call => call.slice(call.indexOf('-s') + 1));

test('an interruption restores the search list without the retired keychain and stops the refresh', async t => {
  const fixture = signingFixture(t);
  const calls = [];
  const signals = new EventEmitter();
  signals.pid = 4242;
  const raised = new Promise(resolve => { signals.kill = (pid, signal) => resolve([pid, signal]); });
  const refresh = fakeProcess();
  let owner;
  let started;
  const running = new Promise(resolve => { started = resolve; });
  const build = withMacosSigning(() => { started(); return new Promise(() => {}); }, fixture.config, {
    exec: mockSecurity(fixture, calls), platform: 'darwin', signals,
    startRefresh: (_, pid) => { owner = pid; return refresh; },
  });
  await running;
  assert.equal(owner, 4242);
  for (const signal of SIGNING_INTERRUPTIONS) assert.equal(signals.listenerCount(signal), 1, signal);
  assert.deepEqual(searchListWrites(calls), [[fixture.keychain, fixture.login]]);
  signals.emit('SIGTERM', 'SIGTERM');
  assert.deepEqual(await raised, [4242, 'SIGTERM']);
  assert.deepEqual(refresh.killed, ['SIGTERM']);
  assert.deepEqual(searchListWrites(calls), [[fixture.keychain, fixture.login], [fixture.login]]);
  for (const signal of SIGNING_INTERRUPTIONS) assert.equal(signals.listenerCount(signal), 0, signal);
  // The password reaches only the unlock command.
  assert.deepEqual(calls.filter(call => call.includes('fixture-keychain-secret')).map(call => call[1]), ['unlock-keychain']);
  assert.equal(calls.some(call => call.includes('fixture-p12-secret')), false);
  void build;
});

test('normal completion restores once and leaves no signal listener behind', t => {
  const fixture = signingFixture(t);
  const calls = [];
  const signals = new EventEmitter();
  signals.pid = 4242;
  signals.kill = () => assert.fail('no signal was received');
  const refresh = fakeProcess();
  const io = { exec: mockSecurity(fixture, calls), platform: 'darwin', signals, startRefresh: () => refresh };
  assert.equal(withMacosSigning(env => env.MIXRELAY_SIGNING_KEYCHAIN, fixture.config, io), fixture.keychain);
  assert.throws(() => withMacosSigning(() => { throw new Error('build failed'); }, fixture.config, io), /build failed/);
  assert.deepEqual(refresh.killed, ['SIGTERM', 'SIGTERM']);
  assert.deepEqual(searchListWrites(calls).filter((_, index) => index % 2 === 1), [[fixture.login], [fixture.login]]);
  for (const signal of SIGNING_INTERRUPTIONS) assert.equal(signals.listenerCount(signal), 0, signal);
});

test('a second signal during cleanup is only forwarded; the first restores the list, awaits the refresh, then re-raises', async t => {
  const fixture = signingFixture(t);
  const calls = [];
  const order = [];
  const security = mockSecurity(fixture, calls);
  const exec = (program, args) => {
    if (args.includes('-s') && !args.includes(fixture.keychain)) order.push('restore');
    return security(program, args);
  };
  const signals = new EventEmitter();
  signals.pid = 4242;
  const raisedSignals = [];
  const raised = new Promise(resolve => { signals.kill = (pid, signal) => { raisedSignals.push(signal); resolve([pid, signal]); }; });
  // The refresh exit is withheld until the test releases it.
  const refresh = new EventEmitter();
  refresh.killed = [];
  const refreshSignalled = new Promise(resolve => { refresh.kill = signal => { refresh.killed.push(signal); order.push('refresh signalled'); resolve(); }; });
  let started;
  const running = new Promise(resolve => { started = resolve; });
  withMacosSigning(() => { started(); return new Promise(() => {}); }, fixture.config, { exec, platform: 'darwin', signals, startRefresh: () => refresh });
  await running;
  signals.emit('SIGTERM', 'SIGTERM');
  await refreshSignalled;
  assert.deepEqual(order, ['restore', 'refresh signalled']);
  for (const signal of SIGNING_INTERRUPTIONS) assert.equal(signals.listenerCount(signal), 1, `${signal} stays handled during cleanup`);
  signals.emit('SIGINT', 'SIGINT');
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(raisedSignals, [], 'nothing is re-raised before the refresh exits');
  assert.deepEqual(refresh.killed, ['SIGTERM']);
  assert.equal(searchListWrites(calls).length, 2);
  refresh.emit('exit', null, 'SIGTERM');
  assert.deepEqual(await raised, [4242, 'SIGTERM']);
  assert.deepEqual(raisedSignals, ['SIGTERM']);
  for (const signal of SIGNING_INTERRUPTIONS) assert.equal(signals.listenerCount(signal), 0, signal);
});

test('an owned command resolves like spawnSync when no signal arrives', async t => {
  const fixture = signingFixture(t);
  const signals = new EventEmitter();
  signals.pid = 4242;
  signals.kill = () => assert.fail('no signal was received');
  const refresh = fakeProcess();
  const result = await withMacosSigning(async (_env, _config, run) => run(process.execPath, ['-e', 'process.exit(3)'], { stdio: 'ignore' }), fixture.config, {
    exec: mockSecurity(fixture, []), platform: 'darwin', signals, startRefresh: () => refresh,
  });
  assert.deepEqual(result, { status: 3, signal: null });
  assert.deepEqual(refresh.killed, ['SIGTERM']);
  for (const signal of SIGNING_INTERRUPTIONS) assert.equal(signals.listenerCount(signal), 0, signal);
});

test('an owned group outliving its leader keeps receiving signals and holds cleanup until it ends', async t => {
  const fixture = signingFixture(t);
  const calls = [];
  const signals = new EventEmitter();
  signals.pid = 4242;
  const raised = new Promise(resolve => { signals.kill = (pid, signal) => resolve([pid, signal]); });
  const refresh = fakeProcess();
  // The build leader (group 777) and the test-controlled liveness of its group.
  const leader = new EventEmitter();
  leader.pid = 777;
  const alive = new Set([777]);
  const forwarded = [];
  let settles = 0;
  let releaseSettle;
  const processGroups = {
    signal: (group, signal) => { if (!alive.has(group)) throw Object.assign(new Error('gone'), { code: 'ESRCH' }); forwarded.push([group, signal]); },
    alive: group => alive.has(group),
    settle: () => { settles += 1; return new Promise(resolve => { releaseSettle = resolve; }); },
  };
  let started;
  const running = new Promise(resolve => { started = resolve; });
  withMacosSigning((_env, _config, run) => { run('build', [], {}); started(); return new Promise(() => {}); }, fixture.config, {
    exec: mockSecurity(fixture, calls), platform: 'darwin', signals, spawnChild: () => leader, processGroups, startRefresh: () => refresh,
  });
  await running;
  const turn = () => new Promise(resolve => setImmediate(resolve));
  signals.emit('SIGINT', 'SIGINT');
  // The leader exits on SIGINT; a descendant keeps its group alive.
  leader.emit('close', null, 'SIGINT');
  await turn();
  assert.equal(settles, 1, 'cleanup awaits the surviving group');
  signals.emit('SIGTERM', 'SIGTERM');
  assert.deepEqual(forwarded, [[777, 'SIGINT'], [777, 'SIGTERM']], 'a repeated signal reaches the group after its leader exited');
  releaseSettle();
  await turn();
  assert.equal(settles, 2, 'the group is still checked while it lives');
  assert.equal(searchListWrites(calls).length, 1, 'the list is not restored while an owned group lives');
  assert.deepEqual(refresh.killed, []);
  alive.delete(777);
  releaseSettle();
  assert.deepEqual(await raised, [4242, 'SIGINT']);
  assert.deepEqual(searchListWrites(calls), [[fixture.keychain, fixture.login], [fixture.login]]);
  assert.deepEqual(refresh.killed, ['SIGTERM']);
  for (const signal of SIGNING_INTERRUPTIONS) assert.equal(signals.listenerCount(signal), 0, signal);
});

// A mocked build (like zsh running cargo) whose blocked grandchild records the signal it
// receives. The build itself ends at once on the signal; the grandchild ends only if the
// wrapper signals the build's whole process group. It records before exiting, so the record
// precedes the group's end whatever the scheduling; a grandchild outliving the build is
// covered deterministically above.
const grandchildSource = `
  const { appendFileSync } = require('node:fs');
  const log = process.argv[1];
  const record = value => appendFileSync(log, JSON.stringify(value) + '\\n');
  record({ grandchild: process.pid });
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => { record({ exit: 'grandchild', signal }); process.exit(0); });
  console.log('ready');
  setInterval(() => {}, 1000);
`;
const buildSource = "require('node:child_process').spawn(process.execPath, ['-e', process.argv[2], process.argv[1]], { stdio: 'inherit' }); setInterval(() => {}, 1000);";
// `process`: the signal reaches only the wrapper PID (kill, launchd). `group`: it reaches the
// wrapper's whole group, as a terminal Ctrl-C does; the build is in its own session, so it
// receives the signal once, through the wrapper.
for (const [signal, delivery] of [['SIGINT', 'group'], ['SIGTERM', 'process'], ['SIGINT', 'process']]) {
  test(`a real ${signal} sent to the ${delivery} stops the build's process tree, restores the list, stops the refresh, then ends by that signal`, { timeout: 20000 }, async t => {
    const fixture = signingFixture(t);
    // The log outlives the fixture directory so the hang guard can still read owned PIDs.
    const logDirectory = mkdtempSync(join(tmpdir(), 'mixrelay-signing-log-'));
    const log = join(logDirectory, 'commands.jsonl');
    const module = pathToFileURL(fileURLToPath(new URL('./macos-signing.mjs', import.meta.url))).href;
    // The wrapper is the parent of the build and the refresh, so it records their exit
    // events itself; the test needs no PID polling or orphan reaping.
    const source = `
      import { appendFileSync } from 'node:fs';
      import { spawn } from 'node:child_process';
      import { withMacosSigning } from ${JSON.stringify(module)};
      const [config, log, original, build, grandchild] = process.argv.slice(1);
      const record = value => appendFileSync(log, JSON.stringify(value) + '\\n');
      const exec = (program, args) => {
        record({ command: [program, ...args.filter(arg => arg !== 'fixture-keychain-secret')] });
        if (args[0] === 'find-identity') return { status: 0, stdout: '"Developer ID Application: Fixture (AB12C3D4E5)" "Developer ID Installer: Fixture (AB12C3D4E5)"' };
        if (args[0] === 'list-keychains' && args.length === 3) return { status: 0, stdout: JSON.parse(original).map(path => JSON.stringify(path)).join('\\n') };
        return { status: 0, stdout: '' };
      };
      const observed = name => (...args) => {
        const child = spawn(...args);
        record({ spawned: name, pid: child.pid });
        child.on('exit', (code, signal) => record({ exit: name, code, signal }));
        return child;
      };
      const startRefresh = () => observed('refresh')(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });
      withMacosSigning(async (_env, _config, run) => {
        await run(process.execPath, ['-e', build, log, grandchild], { stdio: ['ignore', 'inherit', 'inherit'] });
        record({ buildReturned: true });
      }, config, { exec, startRefresh, spawnChild: observed('build'), platform: 'darwin' });
    `;
    const entries = () => readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line));
    // Its own process group lets the hang guard end the wrapper and the refresh together.
    const child = spawn(process.execPath, ['--input-type=module', '-e', source, fixture.config, log, JSON.stringify(fixture.original), buildSource, grandchildSource], { stdio: ['ignore', 'pipe', 'inherit'], detached: true });
    let settled = false;
    t.after(() => {
      // Hang guard: the build leads its own group, so end it and the grandchild separately.
      if (!settled) {
        let owned = [];
        try { owned = entries().flatMap(entry => entry.spawned === 'build' ? [-entry.pid] : entry.grandchild ? [entry.grandchild] : []); } catch { /* nothing was recorded */ }
        for (const pid of [-child.pid, ...owned]) { try { process.kill(pid, 'SIGKILL'); } catch { /* already ended */ } }
      }
      rmSync(logDirectory, { recursive: true, force: true });
    });
    await new Promise((resolve, reject) => {
      child.stdout.on('data', data => { if (String(data).includes('ready')) resolve(); });
      child.on('exit', () => reject(new Error('signing wrapper exited early')));
    });
    if (delivery === 'group') process.kill(-child.pid, signal);
    else child.kill(signal);
    const [code, received] = await once(child, 'exit');
    assert.equal(code, null);
    assert.equal(received, signal);
    const recorded = entries();
    const restore = recorded.findIndex(entry => entry.command?.includes('-s') && entry.command.at(-1) === fixture.login && !entry.command.includes(fixture.keychain));
    const buildExit = recorded.findIndex(entry => entry.exit === 'build');
    const grandchildExit = recorded.findIndex(entry => entry.exit === 'grandchild');
    const refreshExit = recorded.findIndex(entry => entry.exit === 'refresh');
    const writes = recorded.filter(entry => entry.command?.includes('-s')).map(entry => entry.command.slice(entry.command.indexOf('-s') + 1));
    assert.deepEqual(writes, [[fixture.keychain, fixture.login], [fixture.login]]);
    assert.deepEqual(recorded[buildExit], { exit: 'build', code: null, signal });
    assert.ok(buildExit < restore, 'the owned build ends before the search list is restored');
    assert.deepEqual(recorded[grandchildExit], { exit: 'grandchild', signal });
    assert.ok(grandchildExit < restore, "the build's descendants end before the search list is restored");
    const grandchild = recorded.find(entry => entry.grandchild).grandchild;
    assert.throws(() => process.kill(grandchild, 0), { code: 'ESRCH' });
    assert.ok(refreshExit >= 0, 'the wrapper awaits the refresh exit before ending');
    // A terminal signal reaches the refresh directly; otherwise the wrapper stops it after restoring.
    if (delivery === 'process') {
      assert.ok(refreshExit > restore);
      assert.equal(recorded[refreshExit].signal, 'SIGTERM');
    }
    settled = true;
  });
}

const pem = body => `-----BEGIN CERTIFICATE-----\n${Buffer.from(body).toString('base64')}\n-----END CERTIFICATE-----\n`;
function restoreFixture(t, { applicationBody = 'application certificate' } = {}) {
  const fixture = signingFixture(t);
  for (const name of Object.values(SIGNING_BACKUPS)) writeFileSync(join(fixture.directory, name), 'backup', { mode: 0o600 });
  writeFileSync(join(fixture.directory, SIGNING_BACKUPS.installerCertificate), pem('installer certificate'), { mode: 0o600 });
  writeFileSync(join(fixture.directory, SIGNING_BACKUPS.verification), JSON.stringify({
    applicationCertificateSHA1: pemCertificateSha1(pem('application certificate')),
    installerCertificateSHA1: pemCertificateSha1(pem('installer certificate')),
  }), { mode: 0o600 });
  fixture.original = [fixture.login, fixture.keychain, RETIRED_SIGNING_KEYCHAIN];
  const calls = [];
  const security = mockSecurity(fixture, calls);
  const exec = (program, args, options) => {
    if (program === '/usr/bin/openssl') {
      calls.push([program, ...args, options.env.MIXRELAY_P12_PASSWORD]);
      return { status: 0, stdout: pem(applicationBody) };
    }
    return security(program, args);
  };
  const io = { exec: (program, args, options) => (args[0] === 'import' && args[1].endsWith(SIGNING_BACKUPS.authority)
    ? (calls.push([program, ...args]), { status: 1, stderr: 'SecKeychainItemImport: The specified item already exists in the keychain.' })
    : exec(program, args, options)), platform: 'darwin', now: new Date('2026-10-19T01:02:03Z'), password: () => 'fixture-new-keychain-secret' };
  return { ...fixture, calls, io };
}

test('pemCertificateSha1 hashes the DER bytes of the certificate', () => {
  assert.equal(pemCertificateSha1(pem('abc')), 'A9993E364706816ABA3E25717850C26C9CD0D89D');
  assert.throws(() => pemCertificateSha1('no certificate'), /no certificate/);
});

test('keychain restore rebuilds a new build keychain from verified backups and swaps the configuration', t => {
  const fixture = restoreFixture(t);
  const result = restoreMacosSigningKeychain(fixture.config, fixture.io);
  const restored = join(fixture.directory, 'release-20261019-010203.keychain-db');
  assert.deepEqual(result, { keychain: 'release-20261019-010203.keychain-db', previousKeychainKept: 'release.keychain-db', teamId: 'AB12C3D4E5' });
  const config = JSON.parse(readFileSync(fixture.config, 'utf8'));
  assert.equal(config.keychain, restored);
  assert.equal(config.keychainPassword, 'fixture-new-keychain-secret');
  assert.equal(config.p12Password, 'fixture-p12-secret');
  assert.equal(readFileSync(fixture.keychain, 'utf8'), 'not a keychain');
  // The P12 password reaches openssl through the environment, never its arguments.
  const openssl = fixture.calls.find(call => call[0] === '/usr/bin/openssl');
  assert.ok(openssl.slice(1, -1).every(arg => arg !== 'fixture-p12-secret'));
  assert.equal(openssl.at(-1), 'fixture-p12-secret');
  const security = fixture.calls.filter(call => call[0] === '/usr/bin/security').map(call => call[1]);
  assert.deepEqual(security, ['list-keychains', 'create-keychain', 'set-keychain-settings', 'import', 'import', 'import', 'import', 'set-key-partition-list', 'find-identity', 'list-keychains']);
  for (const call of fixture.calls.filter(call => call[0] === '/usr/bin/security' && call[1] !== 'list-keychains')) assert.ok(call.includes(restored), call.join(' '));
  assert.deepEqual(searchListWrites(fixture.calls), [[fixture.login]]);
});

test('keychain restore refuses backups that do not match the verified fingerprints', t => {
  const fixture = restoreFixture(t, { applicationBody: 'another certificate' });
  const before = readFileSync(fixture.config, 'utf8');
  assert.throws(() => restoreMacosSigningKeychain(fixture.config, fixture.io), /verified certificate fingerprints/);
  assert.equal(readFileSync(fixture.config, 'utf8'), before);
  assert.ok(!fixture.calls.some(call => call[0] === '/usr/bin/security'));
});

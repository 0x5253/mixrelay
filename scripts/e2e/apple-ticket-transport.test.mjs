import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { withAppleTicketHttps } from './apple-ticket-transport.mjs';
import { installAndVerify } from './package.mjs';

function fixture(t, overrides = {}) {
  const output = mkdtempSync(join(tmpdir(), 'mr-apple-https-'));
  t.after(() => rmSync(output, { recursive: true, force: true }));
  const calls = [];
  const state = { blocked: false, enabled: false, top: 'anchor "com.apple/*" all\n', existing: '' };
  const options = { platform: 'darwin', arch: 'x64', env: { GITHUB_ACTIONS: 'true' },
    command(program, args) {
      calls.push([program, args]);
      let stdout = '', stderr = 'No ALTQ support in kernel\nALTQ related functions disabled\n';
      if (program === 'sw_vers') stdout = '15.7.9\n';
      else if (args.includes('-f')) {
        assert.equal(state.enabled, false);
        assert.equal(readFileSync(args.at(-1), 'utf8'), 'block return out quick proto udp from any to any port 443\n');
        state.blocked = true;
      } else if (args.includes('-E')) { state.enabled = true; stderr += 'Token : 98765\n'; }
      else if (args.includes('-X')) { assert.equal(args.at(-1), '98765'); state.enabled = false; }
      else if (args.includes('-F')) state.blocked = false;
      else if (args.includes('-sr')) stdout = args.includes('-a')
        ? state.blocked ? 'block return out quick proto udp from any to any port = 443\n' : state.existing
        : state.top;
      return { status: 0, stdout, stderr };
    }, ...overrides };
  return { output, options, state, calls };
}

test('Apple ticket validation uses TCP on affected hosted macOS and restores its own PF state', t => {
  const { output, options, state, calls } = fixture(t);
  const result = withAppleTicketHttps(output, () => {
    assert.ok(state.blocked && state.enabled, 'The affected HTTP/3 route must fall back before validation');
    return 'Apple ticket validated over HTTPS';
  }, options);
  assert.equal(result, 'Apple ticket validated over HTTPS');
  assert.equal(state.blocked, false);
  assert.equal(state.enabled, false);
  assert.ok(calls.filter(([, args]) => args.includes('-f') || args.includes('-F')).every(([, args]) => args.includes('-a')));
  assert.ok(calls.every(([, args]) => !args.includes('-d')));
  const report = JSON.parse(readFileSync(join(output, 'apple-ticket-transport.json')));
  assert.equal(report.cleanup, 'passed');
  assert.equal(report.transport, 'HTTPS over TCP');
});

test('invalid ticket still fails and transport cleanup runs before the failure escapes', t => {
  const { output, options, state } = fixture(t);
  const rejection = new Error('Invalid Apple ticket');
  assert.throws(() => withAppleTicketHttps(output, () => { throw rejection; }, options), error => error === rejection);
  assert.equal(state.blocked, false);
  assert.equal(state.enabled, false);
});

test('the installer validates through the affected route and removes the QA rule before extracting', t => {
  const { output, options, state } = fixture(t);
  let validated = false, installed = false;
  installAndVerify({ file: 'candidate.pkg' }, {}, output, output, 'macos', {
    paths: [], wait: () => {},
    ticketTransport: (directory, action) => withAppleTicketHttps(directory, action, options),
    runCommand(program, args) {
      if (program === 'xcrun') {
        if (!state.blocked || !state.enabled) throw Object.assign(new Error('Apple connection lost'), {
          status: 68, stderr: 'NSURLErrorDomain Code=-1005',
        });
        validated = true;
      }
      if (args[0] === '--expand-full') {
        assert.ok(validated);
        assert.equal(state.blocked, false);
        assert.equal(state.enabled, false);
        mkdirSync(args[2]);
      }
      if (program === 'sudo') installed = true;
      return 'verified';
    },
  });
  assert.ok(installed);
});

test('failed rule cleanup fails the case and still releases the owned enable reference', t => {
  const { output, options, state } = fixture(t);
  const command = options.command;
  options.command = (program, args) => args.includes('-F')
    ? { status: 1, stdout: '', stderr: 'Cannot flush QA rule' } : command(program, args);
  assert.throws(() => withAppleTicketHttps(output, () => 'valid ticket', options), /transport cleanup failed/);
  assert.equal(state.enabled, false);
  const report = JSON.parse(readFileSync(join(output, 'apple-ticket-transport.json')));
  assert.equal(report.cleanup, 'failed');
});

test('a missing platform anchor or an occupied private anchor prevents any network change or validation', t => {
  for (const changed of ['top', 'existing']) {
    const { output, options, state, calls } = fixture(t);
    state[changed] = changed === 'top' ? '' : 'pass all\n';
    let validated = false;
    assert.throws(() => withAppleTicketHttps(output, () => { validated = true; }, options));
    assert.equal(validated, false);
    assert.equal(state.blocked, false);
    assert.equal(state.enabled, false);
    assert.ok(calls.every(([, args]) => !args.includes('-f') && !args.includes('-E')));
  }
});

test('other OS and non-hosted sessions never alter packet filtering', t => {
  for (const override of [{ platform: 'linux' }, { env: {} }]) {
    const { output, options, calls } = fixture(t, override);
    assert.equal(withAppleTicketHttps(output, () => 'unchanged', options), 'unchanged');
    assert.deepEqual(calls, []);
  }
  const { output, options, calls } = fixture(t, { command: (program) => {
    calls.push(program);
    return { status: 0, stdout: '26.6.1\n' };
  } });
  assert.equal(withAppleTicketHttps(output, () => 'unchanged', options), 'unchanged');
  assert.deepEqual(calls, ['sw_vers']);
});

test('the same observed CloudKit failure on macOS 15 ARM uses the scoped TCP path', t => {
  const { output, options, state } = fixture(t, { arch: 'arm64' });
  withAppleTicketHttps(output, () => assert.ok(state.blocked && state.enabled), options);
  assert.equal(state.blocked, false);
  assert.equal(state.enabled, false);
});

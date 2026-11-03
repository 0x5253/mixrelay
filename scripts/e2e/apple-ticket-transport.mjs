import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';

// https://github.com/actions/runner-images/issues/14924
export function withAppleTicketHttps(output, action, { platform = process.platform,
  arch = process.arch, env = process.env, command = spawnSync } = {}) {
  if (platform !== 'darwin' || env.GITHUB_ACTIONS !== 'true') return action();
  const report = { imageVersion: env.ImageVersion, arch, commands: [], cleanup: 'not-required' };
  function run(program, args) {
    const result = command(program, args, { encoding: 'utf8', timeout: 10000, killSignal: 'SIGKILL' });
    report.commands.push({ program, args, status: result.status, stdout: result.stdout,
      stderr: result.stderr, error: result.error?.message });
    assert.ifError(result.error);
    assert.equal(result.status, 0, `${program} ${args.join(' ')} failed`);
    return result;
  }
  const version = run('sw_vers', ['-productVersion']).stdout.trim();
  if (!/^15\./.test(version)) return action();
  report.version = version;
  const anchor = `com.apple/mixrelay-e2e-stapler-${process.pid}`;
  const pf = args => run('sudo', ['-n', 'pfctl', ...args]);
  let loaded = false, enableAttempted = false, token, result, failure;
  const cleanupErrors = [];
  try {
    assert.match(pf(['-sr']).stdout, /^anchor "com\.apple\/\*" all\s*$/m,
      'The existing platform anchor must be active; the main ruleset is never replaced');
    assert.equal(pf(['-a', anchor, '-sr']).stdout.trim(), '', 'The private QA anchor must be empty');
    const rules = join(output, 'apple-ticket-https.pf');
    writeFileSync(rules, 'block return out quick proto udp from any to any port 443\n', { flag: 'wx', mode: 0o600 });
    loaded = true;
    pf(['-a', anchor, '-f', rules]);
    enableAttempted = true;
    const enabled = pf(['-E']);
    token = `${enabled.stdout ?? ''}\n${enabled.stderr ?? ''}`.match(/Token\s*:\s*([0-9]+)/)?.[1];
    assert.ok(token, 'The packet-filter enable reference must have a release token');
    assert.match(pf(['-a', anchor, '-sr']).stdout,
      /block return out quick proto udp from any to any port (?:= )?443/);
    report.transport = 'HTTPS over TCP';
    result = action();
  } catch (error) { failure = error; }
  finally {
    if (loaded) {
      try {
        pf(['-a', anchor, '-F', 'rules']);
        assert.equal(pf(['-a', anchor, '-sr']).stdout.trim(), '', 'The private QA rule must be removed');
      } catch (error) { cleanupErrors.push(error); }
    }
    if (token) {
      try { pf(['-X', token]); } catch (error) { cleanupErrors.push(error); }
    } else if (enableAttempted) {
      cleanupErrors.push(new Error('Packet-filter enable reference could not be confirmed released'));
    }
    report.cleanup = cleanupErrors.length ? 'failed' : 'passed';
    report.error = failure?.message;
    report.cleanupErrors = cleanupErrors.map(error => error.message);
    writeFileSync(join(output, 'apple-ticket-transport.json'), `${JSON.stringify(report, null, 2)}\n`);
  }
  if (cleanupErrors.length) throw new AggregateError([failure, ...cleanupErrors].filter(Boolean), 'Apple ticket transport cleanup failed');
  if (failure) throw failure;
  return result;
}

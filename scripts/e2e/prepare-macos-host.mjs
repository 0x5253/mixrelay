import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// https://github.com/actions/runner-images/issues/14751
export function prepareHostedMacos(output, { command = spawnSync, env = process.env,
  platform = process.platform, arch = process.arch, uid = process.getuid() } = {}) {
  assert.equal(platform, 'darwin');
  assert.equal(env.GITHUB_ACTIONS, 'true', 'Host repair is confined to a disposable GitHub runner');
  mkdirSync(output, { recursive: true });
  const report = { imageOS: env.ImageOS, imageVersion: env.ImageVersion, arch, repaired: false };
  const run = (name, program, args, timeout = 30000) => {
    const result = command(program, args, { encoding: 'utf8', timeout, killSignal: 'SIGKILL', maxBuffer: 4 * 1024 * 1024 });
    writeFileSync(join(output, `${name}.log`), `${result.stdout ?? ''}\n${result.stderr ?? ''}\nstatus=${result.status}; signal=${result.signal ?? ''}; error=${result.error?.code ?? ''}\n`);
    return result;
  };
  const success = (result, description) => {
    assert.ifError(result.error);
    assert.equal(result.status, 0, description);
  };
  try {
    const version = run('os-version', 'sw_vers', ['-productVersion']);
    success(version, 'Cannot identify the hosted macOS version');
    report.version = version.stdout.trim();
    const activityDisabled = text => /Indexing and searching disabled\.|Spotlight server is disabled/i.test(text);
    report.spotlight = { volumes: [] };
    for (const [index, volume] of ['/', '/System/Volumes/Data'].entries()) {
      const before = run(`spotlight-${index}-before`, 'mdutil', ['-s', volume]);
      success(before, 'Cannot inspect Spotlight on the disposable host');
      const text = before.stdout ?? '';
      const wasDisabled = activityDisabled(text);
      const indexingOnly = /Indexing disabled\./.test(text);
      assert.ok(wasDisabled || indexingOnly || /Indexing enabled\./.test(text), 'Unrecognized Spotlight state');
      const observed = { volume, changed: !wasDisabled, activityBefore: wasDisabled ? 'disabled' : indexingOnly ? 'indexing-disabled' : 'enabled' };
      report.spotlight.volumes.push(observed);
      if (!wasDisabled) {
        success(run(`spotlight-${index}-disable`, 'sudo', ['-n', 'mdutil', '-d', volume]), 'Cannot pause disposable-host Spotlight activity');
        const after = run(`spotlight-${index}-after`, 'mdutil', ['-s', volume]);
        success(after, 'Cannot verify disposable-host Spotlight activity');
        assert.ok(activityDisabled(after.stdout ?? ''), 'Spotlight activity must be disabled before the audio session');
      }
      observed.activityAfter = 'disabled';
    }
    if (arch !== 'x64' || !/^26\./.test(report.version)) return report;
    const target = `gui/${uid}/com.apple.iconservices.iconservicesagent`;
    const before = run('icon-agent-before', 'launchctl', ['print', target]);
    const probe = join(output, 'icon-probe');
    success(run('compile-icon-probe', 'xcrun', ['clang', '-fobjc-arc', '-Wall', '-Wextra', '-Werror',
      'scripts/e2e/macos-ui/HostedIconProbe.m', '-framework', 'AppKit', '-framework', 'UniformTypeIdentifiers', '-o', probe], 60000),
    'Cannot compile the native icon probe');
    const first = run('icon-probe-before', probe, [], 15000);
    if (!first.error && first.status === 0) return report;
    assert.equal(first.error?.code, 'ETIMEDOUT', 'An unrelated icon probe failure must not trigger host repair');
    assert.equal(before.status, 0, 'The crashed icon agent must be registered before repair');
    assert.match(before.stdout, /successive crashes = [1-9][0-9]*\b/);
    assert.match(before.stdout, /last terminating signal = Abort trap: 6\b/);
    // Disable only the broken icon renderer; product TCC grants and audio remain untouched.
    run('icon-agent-bootout', 'launchctl', ['bootout', target]);
    success(run('icon-agent-disable', 'launchctl', ['disable', target]), 'Cannot disable the crashing image agent');
    const after = run('icon-agent-after', 'launchctl', ['print', target]);
    assert.notEqual(after.status, 0, 'The crashed icon service must be unloaded');
    assert.ifError(after.error);
    const disabled = run('icon-agent-disabled', 'launchctl', ['print-disabled', `gui/${uid}`]);
    success(disabled, 'Cannot verify the image repair');
    assert.match(disabled.stdout, /"com\.apple\.iconservices\.iconservicesagent"\s*=>\s*(?:true|disabled)\s*(?:\n|$)/);
    success(run('icon-probe-after', probe, [], 15000), 'Native icon rendering must return after host repair');
    report.repaired = true;
    report.iconRendering = 'system fallback after confirmed hosted Metal crash loop';
    return report;
  } finally {
    writeFileSync(join(output, 'host-preparation.json'), `${JSON.stringify(report, null, 2)}\n`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  assert.equal(process.argv.length, 3, 'Usage: node scripts/e2e/prepare-macos-host.mjs OUTPUT_DIRECTORY');
  prepareHostedMacos(resolve(process.argv[2]));
}

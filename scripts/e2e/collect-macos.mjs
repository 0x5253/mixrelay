import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { appendFileSync, closeSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

export function snapshotMacosProcesses(output, run = spawnSync) {
  for (const [file, args] of [
    ['native-probe-processes.txt', ['-A', '-r', '-o', 'pid=,ppid=,state=,%cpu=,rss=,comm=']],
    ['native-probe-threads.txt', ['-A', '-M', '-r', '-o', 'pid=,ppid=,pri=,nice=,state=,comm=']],
  ]) {
    const result = run('ps', args, { encoding: 'utf8', timeout: 10000, maxBuffer: 4 * 1024 * 1024 });
    writeFileSync(join(output, file), `${result.stdout ?? ''}\n${result.stderr ?? ''}\nexit=${result.status}; error=${result.error?.message ?? ''}\n`, { flag: 'wx' });
  }
}

export function collectMacosLoad(output, run = spawnSync) {
  let processes = '';
  for (const [file, program, args] of [
    ['native-failure-host.txt', 'sysctl', ['hw.ncpu', 'hw.logicalcpu', 'hw.physicalcpu', 'hw.memsize', 'machdep.cpu.brand_string']],
    ['native-failure-processes.txt', 'ps', ['-A', '-r', '-o', 'pid=,ppid=,state=,%cpu=,rss=,comm=']],
    ['native-failure-load.txt', 'top', ['-l', '2', '-s', '1', '-o', 'cpu', '-n', '20', '-stats', 'pid,cpu,time,command']],
  ]) {
    const result = run(program, args, { encoding: 'utf8', timeout: 10000, maxBuffer: 4 * 1024 * 1024 });
    writeFileSync(join(output, file), `${result.stdout ?? ''}\n${result.stderr ?? ''}\nexit=${result.status}; error=${result.error?.message ?? ''}\n`, { flag: 'wx' });
    if (program === 'ps' && result.status === 0) processes = result.stdout ?? '';
  }
  // The disposable session can have several WebKit roles; inspect only WebContent.
  const webviews = processes.split('\n').flatMap(line => {
    const match = line.match(/^\s*(\d+)\s+.*\/com\.apple\.WebKit\.WebContent\.xpc\/Contents\/MacOS\/com\.apple\.WebKit\.WebContent\s*$/);
    return match ? [match[1]] : [];
  });
  for (const pid of [...new Set(webviews)].slice(0, 4)) {
    const result = run('sample', [pid, '1', '-file', join(output, `native-failure-webcontent-${pid}.sample.txt`)],
      { encoding: 'utf8', timeout: 10000, maxBuffer: 4 * 1024 * 1024 });
    writeFileSync(join(output, `native-failure-webcontent-${pid}-command.log`),
      `${result.stdout ?? ''}\n${result.stderr ?? ''}\nexit=${result.status}; error=${result.error?.message ?? ''}\n`, { flag: 'wx' });
  }
}

export function collectCoreaudioServers(capture, path, systemLog = '') {
  const servers = capture('coreaudio-server-processes.txt', 'pgrep', ['-fl', 'coreaudiod|Core-Audio-Driver-Service']);
  const pids = new Set((servers.stdout ?? '').split('\n')
    .map(line => line.match(/^(\d+)\s/)?.[1]).filter(Boolean));
  // The remote service executable name differs between macOS versions. The
  // registrar identifies the actual MixRelay host even when pgrep misses it.
  const registered = [...systemLog.matchAll(/Creating remote driver service:[^\n]*MixRelayHAL\.driver[^\n]*\bpid:\s*(\d+)/g)]
    .map(match => match[1]);
  for (const pid of new Set(registered)) {
    const process = capture(`coreaudio-remote-${pid}-process.txt`, 'ps', ['-p', pid, '-o', 'pid=,comm=']);
    // Do not sample a stale registrar PID that has already been reused.
    if (/^\s*\d+\s+.*(?:audio|Audio).*$/m.test(process.stdout ?? '')) pids.add(pid);
  }
  for (const pid of pids) {
    capture(`coreaudio-server-${pid}-sample-command.log`, 'sudo',
      ['-n', 'sample', pid, '1', '-file', path(`coreaudio-server-${pid}.sample.txt`)]);
  }
}

export function collectCoreaudioLog(outputPath, run = spawnSync) {
  const descriptor = openSync(outputPath, 'w');
  let result;
  try {
    // Hosted upgrade logs exceed spawnSync's former 16 MiB pipe limit. Write
    // directly to the evidence file so later permission/publication events survive.
    result = run('log', ['show', '--last', '5m', '--style', 'compact', '--predicate',
      'process CONTAINS[c] "mixrelay" OR process == "coreaudiod" OR process CONTAINS[c] "audio" OR process == "tccd" OR process == "UserNotificationCenter" OR process == "SecurityAgent"'],
    { stdio: ['ignore', descriptor, descriptor], timeout: 30000 });
  } finally { closeSync(descriptor); }
  appendFileSync(outputPath, `\nexit=${result.status}; signal=${result.signal ?? ''}; error=${result.error?.message ?? ''}\n`);
  return readFileSync(outputPath, 'utf8');
}

export function collectSchedulerTrace(capture, path, trace, exists = existsSync) {
  if (!exists(trace)) return;
  const tables = ['process-info', 'thread-info', 'cpu-state', 'thread-state', 'time-sample'];
  const exports = [['toc', ['--toc']], ...tables.map(schema => [schema,
    ['--xpath', `/trace-toc/run[@number="1"]/data/table[@schema="${schema}"]`]])];
  for (const [name, options] of exports) {
    const output = path(`scheduler-trace-${name}.xml`);
    if (!exists(output)) capture(`scheduler-trace-${name}-export.log`, 'xcrun',
      ['xctrace', 'export', '--input', trace, ...options, '--output', output], 60000);
  }
}

export function collectMacos(destination, prefix = '') {
assert.equal(process.platform, 'darwin');
assert.equal(process.env.GITHUB_ACTIONS, 'true');
const output = resolve(destination); mkdirSync(output, { recursive: true });
const path = name => join(output, `${prefix}${name}`);
function capture(file, program, args, timeout = 30000) {
  const result = spawnSync(program, args, { encoding: 'utf8', timeout, maxBuffer: 16 * 1024 * 1024 });
  writeFileSync(path(file), `${result.stdout ?? ''}\n${result.stderr ?? ''}\nexit=${result.status}; signal=${result.signal ?? ''}; error=${result.error?.message ?? ''}\n`);
  return result;
}
collectSchedulerTrace(capture, path, join(output, 'scheduler.trace'));
const logs = join(homedir(), 'Library/Logs/MixRelay');
if (existsSync(logs)) for (const file of readdirSync(logs).filter(name => /^[A-Za-z0-9_.-]+\.log$/.test(name))) {
  copyFileSync(join(logs, file), path(`app-log-${file}`));
}
capture('installer.log', 'sudo', ['-n', 'tail', '-n', '600', '/var/log/install.log']);
const systemLog = collectCoreaudioLog(path('coreaudio-system.log'));
collectCoreaudioServers(capture, path, systemLog);
const brokers = capture('permission-broker-processes.txt', 'pgrep', ['-fl', 'tccd|UserNotificationCenter|SecurityAgent|CoreServicesUIAgent|iconservicesagent|iconservicesd']);
for (const pid of new Set((brokers.stdout ?? '').split('\n').map(line => line.match(/^(\d+)\s/)?.[1]).filter(Boolean))) {
  capture(`permission-broker-${pid}-sample-command.log`, 'sudo',
    ['-n', 'sample', pid, '1', '-file', path(`permission-broker-${pid}.sample.txt`)]);
}
capture('desktop-screen-command.log', 'screencapture', ['-x', path('desktop-screen.png')]);
capture('coreaudio-devices.txt', 'system_profiler', ['SPAudioDataType', '-json']);
capture('coreaudio-probe.txt', 'zsh', ['scripts/probe-macos-roundtrip.sh', '--list']);
for (const [role, path] of [
  ['desktop', '/Applications/MixRelay/MixRelay.app'],
  ['engine', '/Library/Application Support/MixRelay/bin/mixrelay-service'],
  ['driver', '/Library/Application Support/MixRelay/bin/mixrelay-driver-service'],
]) capture(`${role}-entitlements.txt`, 'codesign', ['-d', '--entitlements', ':-', path]);
capture('desktop-windows.txt', 'osascript', ['-e',
  'tell application "System Events" to get {name, name of windows} of (every process whose background only is false)'], 15000);
const processes = capture('product-processes.txt', 'pgrep', ['-u', String(process.getuid()), '-fl', 'mixrelay|MixRelay']);
for (const role of ['engine', 'driver']) {
  const pid = processes.stdout?.split('\n').find(line => line.includes(`/bin/mixrelay-${role}-service`))?.match(/^(\d+) /)?.[1];
  if (pid) capture(`${role}-sample-command.log`, 'sample', [pid, '3', '-file', path(`${role}.sample.txt`)]);
}
for (const line of processes.stdout?.split('\n') ?? []) {
  const pid = line.match(/^(\d+) .*\/Contents\/MacOS\/mixrelay-desktop(?:\s|$)/)?.[1];
  if (pid) capture(`desktop-${pid}-sample-command.log`, 'sample', [pid, '1', '-file', path(`desktop-${pid}.sample.txt`)]);
}
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values } = parseArgs({ options: { output: { type: 'string' } } });
  collectMacos(values.output);
}

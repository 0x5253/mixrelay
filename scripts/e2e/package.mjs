import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import {
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from 'node:fs';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { digest } from '../support-contract.mjs';
import { withAppleTicketHttps } from './apple-ticket-transport.mjs';

const run = (program, args, timeout = 120000) =>
  execFileSync(program, args, { encoding: 'utf8', timeout, maxBuffer: 8 * 1024 * 1024 });
export function spawnLogged(program, args, logPath, { detached = false } = {}) {
  const file = openSync(logPath, 'wx', 0o600);
  try {
    return spawn(program, args, { detached, stdio: ['ignore', file, file] });
  } finally {
    closeSync(file);
  }
}
export const installedPaths = (os) =>
  os === 'macos'
    ? [
        '/Applications/MixRelay/MixRelay.app/Contents/MacOS/mixrelay-desktop',
        '/Library/Application Support/MixRelay/bin/mixrelay-service',
        '/Library/Application Support/MixRelay/bin/mixrelay-driver-service',
        '/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver/Contents/MacOS/MixRelayHAL',
      ]
    : [
        '/usr/bin/mixrelay-desktop',
        '/usr/bin/mixrelay-service',
        '/usr/bin/mixrelay-driver-service',
      ];

// Executables that earlier builds installed and no current package owns. Removal must leave
// none of them, so a stale binary from an install before the engine rename is caught.
export const retiredPaths = (os) =>
  os === 'macos'
    ? ['/Library/Application Support/MixRelay/bin/mixrelay-engine-service']
    : ['/usr/bin/mixrelay-engine-service'];

export function requireDisposableHost(os, acknowledged) {
  assert.equal(
    acknowledged,
    true,
    'Installation requires --disposable-environment on a resettable QA machine',
  );
  assert.notEqual(
    process.getuid(),
    0,
    'Run as the desktop session user, with noninteractive sudo for installation',
  );
  assert.ok(!process.env.MIXRELAY_IPC_DIR, 'Qualification uses the normal installed sockets');
  assert.ok(
    installedPaths(os).every((path) => !existsSync(path)),
    'Reset the QA machine: MixRelay is already installed',
  );
  run('sudo', ['-n', 'true']); // Fail before Installer can open an authentication prompt.
  if (os === 'linux') {
    assert.ok(
      process.env.DISPLAY || process.env.WAYLAND_DISPLAY,
      'A real desktop user session is required',
    );
    run('systemctl', ['--user', 'is-active', 'pipewire.service', 'wireplumber.service']);
  }
}

export function installAndVerify(
  artifact,
  definition,
  directory,
  output,
  os,
  {
    runCommand = run,
    paths = installedPaths(os),
    wait = (milliseconds) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds),
    ticketTransport = withAppleTicketHttps,
    // Called once the installer succeeds, before payload checks that may still fail.
    onInstalled = () => {},
  } = {},
) {
  const run = runCommand;
  const archive = join(directory, artifact.file);
  const extracted = join(output, 'payload');
  const install = (program, args, timeout) => {
    try {
      writeFileSync(join(output, 'package-install.log'), run(program, args, timeout), {
        flag: 'wx',
      });
      onInstalled();
    } catch (error) {
      if (!existsSync(join(output, 'package-install.log')))
        writeFileSync(
          join(output, 'package-install.log'),
          `${error.stdout ?? ''}\n${error.stderr ?? ''}\n${error.message}`,
          { flag: 'wx' },
        );
      throw error;
    }
  };
  if (os === 'macos') {
    const verification = [];
    const verify = (program, args) => {
      for (let attempt = 1; ; attempt++) {
        const entry = { program, args, attempt };
        verification.push(entry);
        let retry = false;
        try {
          entry.stdout = run(program, args);
          entry.status = 0;
          return;
        } catch (error) {
          Object.assign(entry, {
            stdout: String(error.stdout ?? ''), stderr: String(error.stderr ?? ''),
            status: error.status ?? null, signal: error.signal ?? null, error: error.message,
          });
          // Retry only Apple's observed connection-loss error. Invalid or missing
          // tickets and exhausted retries still stop before extraction or install.
          retry = attempt < 3 && program === 'xcrun' && entry.status === 68
            && /NSURLErrorDomain Code=-1005\b/.test(`${entry.stdout}\n${entry.stderr}`);
          if (retry) entry.retryDelayMs = 30000 * 2 ** (attempt - 1);
          if (!retry)
            throw new Error(`${error.message}\n${entry.stdout}\n${entry.stderr}`, { cause: error });
        } finally {
          writeFileSync(join(output, 'package-verification.json'), `${JSON.stringify(verification, null, 2)}\n`);
        }
        if (retry) wait(entry.retryDelayMs);
      }
    };
    verify('pkgutil', ['--check-signature', archive]);
    verify('spctl', ['--assess', '--type', 'install', archive]);
    ticketTransport(output, () => verify('xcrun', ['stapler', 'validate', '-v', archive]));
    run('pkgutil', ['--expand-full', archive, extracted]);
    install('sudo', ['-n', 'installer', '-dumplog', '-pkg', archive, '-target', '/'], 300000);
  } else {
    mkdirSync(extracted);
    if (definition.format === 'deb') {
      run('dpkg-deb', ['--extract', archive, extracted]);
      install(
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
          archive,
        ],
        1200000,
      );
    } else {
      run('bsdtar', ['-xf', archive, '-C', extracted]);
      install('sudo', ['-n', 'pacman', '-U', '--noconfirm', archive], 1200000);
    }
  }
  const entries = readdirSync(extracted, { recursive: true });
  const binaries = paths.map((path) => {
    const sources =
      os === 'macos'
        ? entries.filter(
            (entry) => entry.endsWith(path) && lstatSync(join(extracted, entry)).isFile(),
          )
        : [path.slice(1)];
    assert.equal(sources.length, 1, `Ambiguous or absent package payload: ${path}`);
    assert.ok(lstatSync(path).isFile(), `Installed executable is not a regular file: ${path}`);
    const expected = digest(readFileSync(join(extracted, sources[0])));
    assert.equal(
      digest(readFileSync(path)),
      expected,
      `Installed executable differs from candidate: ${path}`,
    );
    return { path, sha256: expected };
  });
  writeFileSync(
    join(output, 'installed-binaries.json'),
    `${JSON.stringify({ artifact, binaries }, null, 2)}\n`,
    { flag: 'wx' },
  );
}

// Installed Quit drains controls and audio finalization before acknowledging exit.
export async function waitForInstalledExit(observe, { wait = delay, now = () => performance.now(),
  limitMs = 150000 } = {}) {
  const started = now();
  for (;;) {
    const state = observe();
    const elapsedMs = now() - started;
    const stopped = state.runtimes.length === 0 && state.editorClosed;
    if (stopped || elapsedMs >= limitMs) return { ...state, elapsedMs, stopped };
    await wait(Math.min(100, limitMs - elapsedMs));
  }
}

export async function launchInstalled(os, output) {
  if (os === 'macos') {
    run('open', ['-a', '/Applications/MixRelay/MixRelay.app']);
    return async () => {
      const helper =
        '/Applications/MixRelay/Uninstall MixRelay.app/Contents/Helpers/mixrelay-macos-installer';
      let log = '';
      try {
        for (const action of ['quit-graph-app', 'quit-mixrelay-runtime']) {
          log += `${action}\n${run(helper, [action], 150000)}`;
        }
      } catch (error) {
        log += `${error.stdout ?? ''}\n${error.stderr ?? ''}\n${error.message}`;
        throw error;
      } finally {
        writeFileSync(join(output, 'shutdown.log'), log, { flag: 'wx' });
      }
    };
  }
  run('systemctl', ['--user', 'start', 'mixrelay-driver.service']);
  const child = spawnLogged('/usr/bin/mixrelay-desktop', [], join(output, 'desktop.log'));
  let launchError;
  child.on('error', (error) => {
    launchError = error;
  });
  return async () => {
    // This fresh QA user had no installed MixRelay processes before this launch.
    // The editor deliberately leaves its controller running when the editor exits.
    const runtimes = readdirSync('/proc')
      .filter((pid) => /^\d+$/.test(pid))
      .filter((pid) => {
        try {
          return (
            lstatSync(`/proc/${pid}`).uid === process.getuid() &&
            readFileSync(`/proc/${pid}/cmdline`)
              .toString()
              .split('\0')
              .filter(Boolean)
              .join('\0') === '/usr/bin/mixrelay-desktop\0--driver-runtime'
          );
        } catch (error) {
          if (['ENOENT', 'ESRCH'].includes(error.code)) return false;
          throw error;
        }
      });
    try {
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
      for (const pid of runtimes) {
        try {
          process.kill(Number(pid), 'SIGTERM');
        } catch (error) {
          if (error.code !== 'ESRCH') throw error;
        }
      }
      const runtimeAlive = (pid) => {
        try {
          return readFileSync(`/proc/${pid}/cmdline`).length > 0;
        } catch (error) {
          if (['ENOENT', 'ESRCH'].includes(error.code)) return false;
          throw error;
        }
      };
      const observation = await waitForInstalledExit(() => ({
        runtimes: runtimes.filter(runtimeAlive),
        editorClosed: Boolean(launchError || child.exitCode !== null || child.signalCode !== null),
      }));
      writeFileSync(join(output, 'shutdown-observation.json'), `${JSON.stringify(observation, null, 2)}\n`, { flag: 'wx' });
      assert.equal(observation.runtimes.length, 0, 'Installed background runtime did not stop');
      assert.ok(observation.editorClosed, 'Installed app did not stop');
      if (launchError) throw launchError;
    } finally {
      run('systemctl', ['--user', 'stop', 'mixrelay-driver.service']);
    }
  };
}

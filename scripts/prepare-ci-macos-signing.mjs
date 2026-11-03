#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { appendFileSync, chmodSync, existsSync, lstatSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { checkMacosSigning, SIGNING_INTERRUPTIONS, usableKeychainSearchList, withMacosSigning } from './macos-signing.mjs';
import { notarizationArguments } from './macos-notarization.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const secretNames = ['MIXRELAY_MACOS_APPLICATION_P12_BASE64', 'MIXRELAY_MACOS_INSTALLER_P12_BASE64',
  'MIXRELAY_MACOS_P12_PASSWORD', 'MIXRELAY_MACOS_INTERMEDIATE_CERTIFICATE_BASE64',
  'MIXRELAY_NOTARIZATION_KEY_BASE64', 'MIXRELAY_NOTARIZATION_KEY_ID', 'MIXRELAY_NOTARIZATION_ISSUER'];
function ciEnvironment(env, platform) {
  if (platform !== 'darwin' || env.GITHUB_ACTIONS !== 'true' || env.GITHUB_REPOSITORY !== '0x5253/mixrelay') {
    throw new Error('Official macOS signing requires GitHub Actions in 0x5253/mixrelay');
  }
  if (!env.RUNNER_TEMP || !isAbsolute(env.RUNNER_TEMP) || /[\0\r\n]/.test(env.RUNNER_TEMP)) throw new Error('RUNNER_TEMP must be absolute');
  const info = lstatSync(env.RUNNER_TEMP);
  if (!info.isDirectory() || info.isSymbolicLink() || info.uid !== process.getuid()) throw new Error('RUNNER_TEMP must belong to this runner');
}
function security(args, purpose, exec) {
  const result = exec('/usr/bin/security', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 45000 });
  if (result.error || result.status !== 0 || result.signal) throw new Error(`macOS CI signing could not ${purpose}`);
  return result.stdout?.trim() ?? '';
}
function decode(env, name, maximum) {
  const value = env[name];
  if (typeof value !== 'string' || !value || value.length > maximum * 2 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) {
    throw new Error(`Missing or invalid base64 secret: ${name}`);
  }
  const bytes = Buffer.from(value, 'base64');
  if (!bytes.length || bytes.length > maximum || bytes.toString('base64') !== value) throw new Error(`Invalid base64 secret: ${name}`);
  return bytes;
}
function pathsFrom(text) {
  const paths = text.split('\n').filter(line => line.trim()).map(line => JSON.parse(line.trim()));
  if (paths.some(path => typeof path !== 'string' || !isAbsolute(path) || /[\0\r\n]/.test(path))) throw new Error('Invalid runner keychain search list');
  return usableKeychainSearchList(paths);
}
function privateDirectory(directory, env) {
  const path = resolve(directory);
  if (dirname(path) !== resolve(env.RUNNER_TEMP) || !/^mixrelay-signing-[A-Za-z0-9]+$/.test(path.slice(dirname(path).length + 1))) throw new Error('Refusing cleanup outside the CI signing directory');
  const info = lstatSync(path);
  if (!info.isDirectory() || info.isSymbolicLink() || info.uid !== process.getuid() || info.mode & 0o077) throw new Error('CI signing directory must be private to its owner');
  return path;
}

export function cleanupCIMacosSigning(directory, { env = process.env, exec = spawnSync, platform = process.platform } = {}) {
  ciEnvironment(env, platform);
  if (!directory || !existsSync(directory)) return;
  const path = privateDirectory(directory, env);
  const statePath = join(path, 'search-list.json');
  const info = lstatSync(statePath);
  if (!info.isFile() || info.isSymbolicLink() || info.uid !== process.getuid() || info.mode & 0o077) throw new Error('CI signing cleanup state must be owner-only');
  const original = pathsFrom(readFileSync(statePath, 'utf8'));
  const keychain = join(path, 'release.keychain-db');
  let failure;
  try { security(['list-keychains', '-d', 'user', '-s', ...original], 'restore the runner keychain search list', exec); }
  catch (error) { failure = error; }
  try {
    if (existsSync(keychain)) security(['delete-keychain', keychain], 'delete the temporary build keychain', exec);
  } catch (error) { failure ??= error; }
  finally { rmSync(path, { recursive: true, force: true }); }
  if (failure) throw failure;
}

/** Import only CI-provided assets; existing developer signing configuration is never read. */
export async function withCIMacosSigning(action, {
  env = process.env, exec = spawnSync, platform = process.platform, signals = process,
  preflight = checkMacosSigning, sign = withMacosSigning,
  spawnRefresh = (path, owner) => spawn(process.execPath,
    [join(root, 'scripts/macos-signing.mjs'), '--keep-unlocked', path, String(owner)],
    { stdio: ['ignore', 'ignore', 'inherit'] }),
} = {}) {
  ciEnvironment(env, platform);
  const application = decode(env, 'MIXRELAY_MACOS_APPLICATION_P12_BASE64', 1024 * 1024);
  const installer = decode(env, 'MIXRELAY_MACOS_INSTALLER_P12_BASE64', 1024 * 1024);
  const key = decode(env, 'MIXRELAY_NOTARIZATION_KEY_BASE64', 16 * 1024);
  const intermediate = env.MIXRELAY_MACOS_INTERMEDIATE_CERTIFICATE_BASE64
    ? decode(env, 'MIXRELAY_MACOS_INTERMEDIATE_CERTIFICATE_BASE64', 64 * 1024) : undefined;
  const password = env.MIXRELAY_MACOS_P12_PASSWORD;
  const teamId = env.MIXRELAY_MACOS_TEAM_ID;
  if (typeof password !== 'string' || !password || password.includes('\0')) throw new Error('MIXRELAY_MACOS_P12_PASSWORD is required');
  if (!/^[A-Z0-9]{10}$/.test(teamId ?? '')
    || !env.MIXRELAY_MACOS_APPLICATION_IDENTITY?.startsWith('Developer ID Application: ')
    || !env.MIXRELAY_MACOS_INSTALLER_IDENTITY?.startsWith('Developer ID Installer: ')
    || [env.MIXRELAY_MACOS_APPLICATION_IDENTITY, env.MIXRELAY_MACOS_INSTALLER_IDENTITY].some(value => /[\0\r\n]/.test(value) || !value.endsWith(`(${teamId})`))) {
    throw new Error('Matching Developer ID Application/Installer identities and Team ID are required');
  }
  const notarization = { apiKeyPath: join(resolve(env.RUNNER_TEMP), 'AuthKey.p8'), keyId: env.MIXRELAY_NOTARIZATION_KEY_ID, issuer: env.MIXRELAY_NOTARIZATION_ISSUER };
  notarizationArguments(notarization);
  const original = pathsFrom(security(['list-keychains', '-d', 'user'], 'read the runner keychain search list', exec));
  const directory = mkdtempSync(join(env.RUNNER_TEMP, 'mixrelay-signing-'));
  chmodSync(directory, 0o700);
  let interrupted;
  const observe = signal => { interrupted ??= signal; };
  const listeners = SIGNING_INTERRUPTIONS.map(signal => [signal, () => observe(signal)]);
  for (const [signal, listener] of listeners) signals.on(signal, listener);
  const ownedSignals = { pid: signals.pid,
    on: (signal, listener) => signals.on(signal, listener),
    removeListener: (signal, listener) => signals.removeListener(signal, listener),
    kill: (_pid, signal) => { throw new Error(`macOS CI signing interrupted by ${signal}`); },
  };
  const refreshes = [];
  const startRefresh = (path, owner) => {
    const child = spawnRefresh(path, owner);
    const completion = new Promise(resolve => {
      child.once('error', () => { signals.emit('SIGTERM'); resolve(); });
      child.once('close', (code, signal) => {
        if (code !== 0 && signal !== 'SIGTERM') signals.emit('SIGTERM');
        resolve();
      });
    });
    refreshes.push({ child, completion });
    return child;
  };
  const signingIO = { signals: ownedSignals, startRefresh };
  const run = (args, purpose) => {
    if (interrupted) throw new Error(`macOS CI signing interrupted by ${interrupted}`);
    return security(args, purpose, exec);
  };
  const privateFile = (name, bytes) => writeFileSync(join(directory, name), bytes, { mode: 0o600, flag: 'wx' });
  let saved = false;
  try {
    privateFile('search-list.json', original.map(path => JSON.stringify(path)).join('\n'));
    saved = true;
    if (env.GITHUB_ENV) appendFileSync(env.GITHUB_ENV, `MIXRELAY_CI_SIGNING_DIRECTORY=${directory}\n`);
    privateFile('application.p12', application);
    privateFile('installer.p12', installer);
    privateFile('AuthKey.p8', key);
    if (intermediate) privateFile('intermediate.cer', intermediate);
    const keychain = join(directory, 'release.keychain-db');
    const keychainPassword = randomBytes(32).toString('base64url');
    run(['create-keychain', '-p', keychainPassword, keychain], 'create the temporary build keychain');
    run(['set-keychain-settings', '-lut', '300', keychain], 'set the temporary keychain lock interval');
    run(['unlock-keychain', '-p', keychainPassword, keychain], 'unlock the temporary build keychain');
    for (const name of ['application.p12', 'installer.p12']) {
      run(['import', join(directory, name), '-k', keychain, '-f', 'pkcs12', '-P', password,
        '-T', '/usr/bin/codesign', '-T', '/usr/bin/pkgbuild', '-T', '/usr/bin/productbuild'], 'import the Developer ID signing asset');
    }
    if (intermediate) run(['import', join(directory, 'intermediate.cer'), '-k', keychain, '-t', 'cert'], 'import the Developer ID intermediate certificate');
    run(['set-key-partition-list', '-S', 'apple-tool:,apple:,codesign:', '-s', '-k', keychainPassword, keychain], 'authorize Apple signing tools without a dialog');
    const config = { keychain, keychainPassword, p12Password: password, teamId,
      applicationIdentity: env.MIXRELAY_MACOS_APPLICATION_IDENTITY, installerIdentity: env.MIXRELAY_MACOS_INSTALLER_IDENTITY,
      notarization: { ...notarization, apiKeyPath: join(directory, 'AuthKey.p8') } };
    privateFile('config.json', JSON.stringify(config));
    const configPath = join(directory, 'config.json');
    await preflight(configPath, signingIO);
    if (interrupted) throw new Error(`macOS CI signing interrupted by ${interrupted}`);
    return await sign((signingEnvironment, configuration, owned) => {
      const clean = { ...signingEnvironment, MIXRELAY_SIGNING_CONFIG: configPath };
      for (const name of secretNames) delete clean[name];
      return action(clean, configuration, owned);
    }, configPath, signingIO);
  } finally {
    try {
      try {
        for (const { child } of refreshes) {
          if (child.exitCode == null && child.signalCode == null) child.kill('SIGTERM');
        }
        await Promise.all(refreshes.map(({ completion }) => completion));
      } finally {
        if (saved) cleanupCIMacosSigning(directory, { env, exec, platform });
        else rmSync(directory, { recursive: true, force: true });
      }
    } finally {
      for (const [signal, listener] of listeners) signals.removeListener(signal, listener);
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const { values } = parseArgs({ options: { output: { type: 'string' }, cleanup: { type: 'boolean' } } });
    if (values.cleanup && !values.output) cleanupCIMacosSigning(process.env.MIXRELAY_CI_SIGNING_DIRECTORY);
    else if (values.output && !values.cleanup) {
      const output = resolve(values.output);
      await withCIMacosSigning(async (env, _config, run) => {
        const result = await run(process.execPath, ['scripts/release-macos.mjs', '--output', output], { cwd: root, env, stdio: 'inherit' });
        if (result.error || result.status !== 0 || result.signal) throw new Error('Official macOS release build failed');
      });
    } else throw new Error('Use --output EMPTY_DIRECTORY or --cleanup');
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}

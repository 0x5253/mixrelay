#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { createHash, randomBytes } from 'node:crypto';
import { once } from 'node:events';
import { basename, dirname, join, resolve } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const DEFAULT_SIGNING_CONFIG = join(homedir(), 'Library/Application Support/MixRelay/Signing/background-stop-20261011/config.json');
export const RETIRED_SIGNING_KEYCHAIN = join(homedir(), 'Library/Caches/io.mixrelay.signing/driver-20261008/release.keychain-db');
const root = fileURLToPath(new URL('..', import.meta.url));
function privateFile(path) {
  const value = lstatSync(path);
  if (!value.isFile() || value.isSymbolicLink() || value.uid !== process.getuid() || (value.mode & 0o077)) throw new Error('Signing configuration must be a regular owner-only file');
}
/** Read credential references without opening a keychain or reading private keys. */
export function readMacosSigningConfig(configPath = process.env.MIXRELAY_SIGNING_CONFIG || DEFAULT_SIGNING_CONFIG) {
  privateFile(configPath);
  const text = readFileSync(configPath, 'utf8');
  try { return JSON.parse(text); }
  catch { throw new Error('Signing configuration must contain valid JSON'); }
}
function command(program, args, secrets = [], exec = spawnSync, env = process.env) {
  const result = exec(program, args, { encoding: 'utf8', env, timeout: 45000, maxBuffer: 1024 * 1024 });
  if (result.error || result.status !== 0) {
    let message = result.error?.message || result.stderr || 'Signing command failed';
    for (const secret of secrets.filter(Boolean)) message = message.replaceAll(secret, '[redacted]');
    throw new Error(`${program}: ${message.trim()}`);
  }
  return result.stdout.trim();
}
/** Preserve unrelated keychains while excluding the known unusable signing copy. */
export function usableKeychainSearchList(paths) {
  return paths.filter(path => resolve(path) !== RETIRED_SIGNING_KEYCHAIN);
}
function searchList(exec = spawnSync) {
  return command('/usr/bin/security', ['list-keychains', '-d', 'user'], [], exec).split('\n').filter(Boolean).map(line => JSON.parse(line.trim()));
}
/** Interruptions that must not skip restoring the search list or leave the refresh running. */
export const SIGNING_INTERRUPTIONS = ['SIGINT', 'SIGTERM', 'SIGHUP'];
function spawnRefresh(configPath, owner) {
  return spawn(process.execPath, [fileURLToPath(import.meta.url), '--keep-unlocked', configPath, String(owner)], { stdio: ['ignore', 'ignore', 'inherit'] });
}
/** Remove only the retired lookup entry; never delete keychain files or change lock policy. */
export function repairSigningSearchList() {
  const original = searchList();
  const usable = usableKeychainSearchList(original);
  if (usable.length !== original.length) command('/usr/bin/security', ['list-keychains', '-d', 'user', '-s', ...usable]);
  return { retiredEntryRemoved: usable.length !== original.length, remainingKeychains: usable.length };
}
/**
 * Owned process groups of this machine. A group lives while any member is left (zombies
 * included); `settle` paces liveness checks while cleanup awaits a group's end.
 */
const systemProcessGroups = {
  signal: (group, signal) => process.kill(-group, signal),
  alive: group => {
    try { process.kill(-group, 0); return true; } catch (error) { return error.code !== 'ESRCH'; }
  },
  settle: () => new Promise(resolve => setTimeout(resolve, 50)),
};
/**
 * Unlock the restored keychain; restore unrelated search entries on every exit.
 * `action(env, config, run)` receives `run(program, args, options)`, which starts an owned
 * command asynchronously and resolves like `spawnSync` ({ status, signal, error }). Each
 * owned command leads its own process group (and session), so the build's descendants
 * (zsh → pnpm, cargo, codesign) are reached as one unit and a terminal Ctrl-C reaches the
 * build only through this wrapper, once. Long build steps use it so that SIGINT, SIGTERM
 * and SIGHUP can be handled while they run: the signal is forwarded to every owned process
 * group, the wrapper awaits the end of those groups, restores the list, stops the refresh
 * and awaits its exit, then re-raises the first signal for its default action. A group stays
 * owned until its end is confirmed, even after its leader exits, so a repeated signal still
 * reaches descendants that outlive the leader. The signal listeners stay installed until
 * then, so a repeated signal is only forwarded again and cannot cut cleanup short. Work done
 * synchronously inside the action (`spawnSync`) receives the signal only after it returns
 * (Node runs signal handlers between tasks); its normal cleanup then applies. `io` replaces
 * the command runner, owned process spawner, process groups, refresh process, platform and
 * signal target in tests.
 */
export function withMacosSigning(action, configPath = process.env.MIXRELAY_SIGNING_CONFIG || DEFAULT_SIGNING_CONFIG, io = {}) {
  const { exec = spawnSync, spawnChild = spawn, processGroups = systemProcessGroups, startRefresh = spawnRefresh, platform = process.platform, signals = process } = io;
  if (platform !== 'darwin') throw new Error('macOS signing requires macOS');
  const config = readMacosSigningConfig(configPath);
  const keychain = resolve(config.keychain);
  if (dirname(keychain) !== dirname(resolve(configPath)) || !existsSync(keychain)) throw new Error('Use the restored build keychain next to its configuration');
  if (!/^[A-Z0-9]{10}$/.test(config.teamId) || !config.applicationIdentity?.endsWith(`(${config.teamId})`) || !config.installerIdentity?.endsWith(`(${config.teamId})`)) throw new Error('Matching Application and Installer signing identities required');
  const secrets = [config.keychainPassword, config.p12Password];
  command('/usr/bin/security', ['unlock-keychain', '-p', config.keychainPassword, keychain], secrets, exec);
  const identities = command('/usr/bin/security', ['find-identity', '-v', keychain], [], exec);
  for (const identity of [config.applicationIdentity, config.installerIdentity]) {
    if (!identities.includes(`"${identity}"`)) throw new Error('Restored keychain is missing a required release identity');
  }
  // The retired release keychain has a known stale password. It is never restored, so it
  // cannot shadow the restored private keys in this or any later build.
  const original = usableKeychainSearchList(searchList(exec));
  const active = [keychain, ...original.filter(path => path !== keychain)];
  let refresh;
  let cleaned = false;
  let interruption;
  /** Owned commands still running, each with its completion. */
  const children = new Map();
  /** Owned process groups not yet confirmed ended; a leader's exit does not end its group. */
  const groups = new Set();
  const run = (program, args, options) => {
    if (interruption) return Promise.resolve({ status: null, signal: null, error: new Error('Signing build was interrupted') });
    const child = spawnChild(program, args, { ...options, detached: true });
    if (child.pid !== undefined) groups.add(child.pid);
    const completion = new Promise(resolve => {
      child.once('error', error => resolve({ status: null, signal: null, error }));
      child.once('close', (status, signal) => resolve({ status, signal }));
    });
    children.set(child, completion);
    completion.then(() => {
      children.delete(child);
      if (child.pid !== undefined && !processGroups.alive(child.pid)) groups.delete(child.pid);
    });
    return completion;
  };
  const listen = () => { for (const signal of SIGNING_INTERRUPTIONS) signals.on(signal, interrupted); };
  const unlisten = () => { for (const signal of SIGNING_INTERRUPTIONS) signals.removeListener(signal, interrupted); };
  const restoreSearchList = () => command('/usr/bin/security', ['list-keychains', '-d', 'user', '-s', ...original], [], exec);
  /** Normal exit: restore the list, then stop the refresh; nothing is left listening. */
  const cleanup = () => {
    if (cleaned) return;
    cleaned = true;
    try { restoreSearchList(); } finally {
      refresh?.kill('SIGTERM');
      unlisten();
    }
  };
  function interrupted(signal) {
    // Every signal reaches every owned group; the first one owns cleanup and the re-raise.
    for (const group of groups) {
      try { processGroups.signal(group, signal); } catch (error) { if (error.code === 'ESRCH') groups.delete(group); }
    }
    interruption ??= (async () => {
      while (children.size) await Promise.all(children.values());
      for (const group of groups) {
        while (processGroups.alive(group)) await processGroups.settle();
        groups.delete(group);
      }
      cleaned = true;
      try { restoreSearchList(); } catch (error) { console.error(`Signing cleanup after ${signal}: ${error.message}`); }
      if (refresh && refresh.exitCode == null && refresh.signalCode == null) {
        const refreshExited = once(refresh, 'exit');
        refresh.kill('SIGTERM');
        await refreshExited.catch(() => { /* a failed kill emits 'error'; cleanup must not stall on it */ });
      }
      unlisten();
      // Our listeners are gone, so the re-raised signal takes its default action.
      signals.kill(signals.pid, signal);
    })();
  }
  // Listen before changing the list: without a listener a signal ends Node at once.
  listen();
  let pending = false;
  try {
    command('/usr/bin/security', ['list-keychains', '-d', 'user', '-s', ...active], [], exec);
    // Compilation can outlast the keychain's 300-second idle lock. Refresh only this
    // restored build keychain while its owning build exists; keep OS policy.
    refresh = startRefresh(resolve(configPath), signals.pid);
    const result = action({ ...process.env, APPLE_SIGNING_IDENTITY: config.applicationIdentity, MIXRELAY_CODESIGN_IDENTITY: config.applicationIdentity, MIXRELAY_INSTALLER_IDENTITY: config.installerIdentity, MIXRELAY_SIGNING_KEYCHAIN: keychain }, config, run);
    if (result && typeof result.then === 'function') {
      pending = true;
      // After an interruption the signal path finishes cleanup and ends the process.
      return Promise.resolve(result).finally(() => interruption ?? cleanup());
    }
    return result;
  } finally {
    if (!pending) cleanup();
  }
}
/** Backups next to the configuration, as restored from the signing operations page. */
export const SIGNING_BACKUPS = {
  application: 'application.p12',
  authority: 'DeveloperIDG2CA.cer',
  installerCertificate: 'installer-cert.pem',
  installerKey: 'installer-key.pem',
  verification: 'backup-verification.json',
};
/** SHA-1 of the first PEM certificate's DER bytes, as `security` and Apple report it. */
export function pemCertificateSha1(pem) {
  const body = /-----BEGIN CERTIFICATE-----([^-]+)-----END CERTIFICATE-----/.exec(pem)?.[1];
  if (!body) throw new Error('Signing backup contains no certificate');
  return createHash('sha1').update(Buffer.from(body.replace(/\s+/g, ''), 'base64')).digest('hex').toUpperCase();
}
/**
 * Rebuild the dedicated build keychain from the verified backups next to the configuration,
 * for when its stored password stops unlocking it. Creates a new keychain with a new password
 * (300-second idle lock, keys usable by Apple signing tools only), then replaces the
 * configuration atomically. The login keychain, lock policy and the unusable keychain file
 * stay untouched; only its search-list entry is dropped so no lookup can prompt for it.
 * `io` replaces the command runner, platform, clock and password source in tests.
 */
export function restoreMacosSigningKeychain(configPath = process.env.MIXRELAY_SIGNING_CONFIG || DEFAULT_SIGNING_CONFIG, io = {}) {
  const { exec = spawnSync, platform = process.platform, now = new Date(), password = () => randomBytes(32).toString('base64url') } = io;
  if (platform !== 'darwin') throw new Error('macOS signing requires macOS');
  const config = readMacosSigningConfig(configPath);
  const directory = dirname(resolve(configPath));
  const backup = Object.fromEntries(Object.entries(SIGNING_BACKUPS).map(([key, name]) => [key, join(directory, name)]));
  for (const path of Object.values(backup)) privateFile(path);
  const expected = JSON.parse(readFileSync(backup.verification, 'utf8'));
  const p12Env = { ...process.env, MIXRELAY_P12_PASSWORD: config.p12Password };
  // LibreSSL reads the backup's legacy PKCS#12 encryption without extra providers.
  const application = command('/usr/bin/openssl', ['pkcs12', '-in', backup.application, '-nokeys', '-clcerts', '-passin', 'env:MIXRELAY_P12_PASSWORD'], [config.p12Password], exec, p12Env);
  if (pemCertificateSha1(application) !== expected.applicationCertificateSHA1 || pemCertificateSha1(readFileSync(backup.installerCertificate, 'utf8')) !== expected.installerCertificateSHA1) {
    throw new Error('Signing backups do not match the verified certificate fingerprints');
  }
  const stamp = now.toISOString().replace(/[-:]/g, '').replace('T', '-').slice(0, 15);
  const keychain = join(directory, `release-${stamp}.keychain-db`);
  if (existsSync(keychain)) throw new Error('A restored keychain for this time already exists');
  const keychainPassword = password();
  const secrets = [keychainPassword, config.p12Password];
  const previous = resolve(config.keychain);
  const original = searchList(exec);
  const run = args => command('/usr/bin/security', args, secrets, exec);
  try {
    run(['create-keychain', '-p', keychainPassword, keychain]);
    run(['set-keychain-settings', '-lut', '300', keychain]);
    run(['import', backup.application, '-k', keychain, '-f', 'pkcs12', '-P', config.p12Password, '-T', '/usr/bin/codesign']);
    // The P12 may already carry the intermediate; a duplicate certificate is the same trust.
    for (const certificate of [backup.authority, backup.installerCertificate]) {
      try { run(['import', certificate, '-k', keychain, '-t', 'cert']); }
      catch (error) { if (!/already exists/.test(error.message)) throw error; }
    }
    run(['import', backup.installerKey, '-k', keychain, '-t', 'priv', '-T', '/usr/bin/pkgbuild', '-T', '/usr/bin/productbuild']);
    // Signing tools then use the keys without an access prompt.
    run(['set-key-partition-list', '-S', 'apple-tool:,apple:,codesign:', '-s', '-k', keychainPassword, keychain]);
    const identities = run(['find-identity', '-v', keychain]);
    for (const identity of [config.applicationIdentity, config.installerIdentity]) {
      if (!identities.includes(`"${identity}"`)) throw new Error('Restored keychain is missing a required release identity');
    }
  } catch (error) {
    try { run(['delete-keychain', keychain]); } catch { /* creation may have failed before the file existed */ }
    throw error;
  } finally {
    // create-keychain adds itself to the list; builds add the active keychain only while they run.
    const kept = usableKeychainSearchList(original).filter(path => resolve(path) !== previous && resolve(path) !== keychain);
    run(['list-keychains', '-d', 'user', '-s', ...kept]);
  }
  const staged = `${configPath}.restoring`;
  writeFileSync(staged, `${JSON.stringify({ ...config, keychain, keychainPassword }, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
  renameSync(staged, configPath);
  return { keychain: basename(keychain), previousKeychainKept: basename(previous), teamId: config.teamId };
}
/** Actual code and PKG signing probe; does not install, change ACLs, or publish anything. */
export function checkMacosSigning(configPath, io = {}) {
  return withMacosSigning((env, config) => {
    const proof = join(root, '.work/signing-preflight');
    mkdirSync(proof, { recursive: true });
    const executable = join(proof, 'probe');
    copyFileSync('/usr/bin/true', executable);
    command('/usr/bin/codesign', ['--force', '--sign', env.APPLE_SIGNING_IDENTITY, '--keychain', env.MIXRELAY_SIGNING_KEYCHAIN, '--options', 'runtime', '--timestamp', '--identifier', 'io.mixrelay.signing.preflight', executable]);
    command('/usr/bin/codesign', ['--verify', '--strict', executable]);
    const pkg = join(proof, 'probe.pkg');
    if (existsSync(pkg)) rmSync(pkg);
    command('/usr/bin/pkgbuild', ['--nopayload', '--identifier', 'io.mixrelay.signing.preflight', '--version', '0.1.0', '--sign', env.MIXRELAY_INSTALLER_IDENTITY, '--keychain', env.MIXRELAY_SIGNING_KEYCHAIN, '--timestamp', pkg]);
    const signature = command('/usr/sbin/pkgutil', ['--check-signature', pkg]);
    if (!signature.includes(env.MIXRELAY_INSTALLER_IDENTITY) || !signature.includes('trusted')) throw new Error('PKG signature did not pass the release identity gate');
    return { keychainUnlocked: true, applicationSignatureVerified: true, installerSignatureVerified: true, teamId: config.teamId };
  }, configPath, io);
}
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try {
    if (process.argv[2] === '--keep-unlocked') {
      const configPath = process.argv[3];
      const owner = Number(process.argv[4]);
      if (!Number.isSafeInteger(owner) || owner <= 1) throw new Error('Signing refresh requires its build process owner');
      privateFile(configPath);
      const config = JSON.parse(readFileSync(configPath, 'utf8'));
      const refresh = () => {
        try { process.kill(owner, 0); }
        catch { process.exit(0); }
        command('/usr/bin/security', ['unlock-keychain', '-p', config.keychainPassword, config.keychain], [config.keychainPassword, config.p12Password]);
      };
      refresh();
      setInterval(() => { try { refresh(); } catch (error) { console.error(error.message); process.exit(1); } }, 45000);
    } else if (process.argv[2] === '--repair-search-list') console.log(JSON.stringify(repairSigningSearchList()));
    else if (process.argv[2] === '--restore-keychain') console.log(JSON.stringify(restoreMacosSigningKeychain()));
    else if (process.argv[2] === '--help') console.log('pnpm signing:check [--repair-search-list | --restore-keychain]\nUnlocks the restored MixRelay build keychain, signs/verifies code and PKG probes, and restores unrelated keychain search entries. The retired signing copy is excluded on exit.\n--repair-search-list removes only that stale lookup entry without signing, deleting keys or changing lock policy.\n--restore-keychain rebuilds the build keychain from the verified backups next to the configuration with a new password, when the stored one no longer unlocks it. Run pnpm signing:check afterwards.\nMIXRELAY_SIGNING_CONFIG selects a private local configuration. Passwords never enter repository files or logs.');
    else if (process.argv.length === 2) console.log(JSON.stringify(checkMacosSigning()));
    else throw new Error('Use pnpm signing:check or --help');
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}

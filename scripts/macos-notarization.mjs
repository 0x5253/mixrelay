import { spawnSync } from 'node:child_process';
import * as fsApi from 'node:fs';
import { dirname, isAbsolute, join } from 'node:path';

const UUID = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i;
const reference = value => typeof value === 'string' && value.trim().length > 0 && !/[\0\r\n]/.test(value);

/** Submission waits at most this long for Apple's verdict; the process gets a margin. */
const SUBMIT_WAIT = '2h';
const SUBMIT_TIMEOUT_MS = (2 * 60 + 10) * 60 * 1000;
const TOOL_TIMEOUT_MS = 10 * 60 * 1000;

/**
 * Accept App Store Connect API key references only; never load key contents or accept
 * inline passwords. notarytool reads the key file directly, so no keychain item is
 * consulted and no authentication dialog can open during a release.
 */
export function notarizationArguments(credentials, skipNotarization = false) {
  if (skipNotarization === true) return null;
  const invalid = () => new Error('Signing config requires notarization.apiKeyPath + keyId + issuer (App Store Connect API key); use --skip-notarization only for local test builds');
  if (!credentials || typeof credentials !== 'object' || Array.isArray(credentials)) throw invalid();
  const keys = Object.keys(credentials).sort().join(',');
  if (keys === 'apiKeyPath,issuer,keyId' && reference(credentials.apiKeyPath) && isAbsolute(credentials.apiKeyPath)
    && /^[A-Z0-9]{10}$/.test(credentials.keyId) && UUID.test(credentials.issuer)) {
    return ['--key', credentials.apiKeyPath, '--key-id', credentials.keyId, '--issuer', credentials.issuer];
  }
  throw invalid();
}

/**
 * Runs inside the signing wrapper's owned packaging process group. Synchronous xcrun
 * children inherit that group, so interruption stops submission, logging and stapling.
 * Persist each pending step before starting it; abrupt termination never leaves success.
 */
export function notarizePackage(result, authentication, { fs = fsApi, run = spawnSync } = {}) {
  const outcome = result.notarization;
  const output = dirname(result.installer);
  const resultPath = join(output, 'build-result.json');
  const save = () => fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600 });
  const invoke = (args, timeout = TOOL_TIMEOUT_MS) => {
    // Capture tool output: diagnostics can echo credential references. Only selected
    // response fields and the redacted notarization log become build artifacts.
    try {
      return run('/usr/bin/xcrun', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 4 * 1024 * 1024, timeout });
    } catch {
      return { status: null, error: true };
    }
  };
  // Apple's log lists warnings even for accepted submissions, so it is kept either way.
  const fetchLog = () => {
    const log = invoke(['notarytool', 'log', outcome.submissionId, ...authentication]);
    outcome.logStatus = succeeded(log) ? 'fetched' : log?.signal ? 'interrupted' : 'failed';
    if (succeeded(log)) {
      const path = join(output, 'notarization-log.json');
      let text = String(log.stdout ?? '');
      for (const value of authentication.filter(value => !value.startsWith('--'))) text = text.replaceAll(value, '[redacted]');
      fs.writeFileSync(path, text, { mode: 0o600 });
      outcome.logPath = path;
    }
    return log;
  };
  const succeeded = response => response && response.status === 0 && !response.error && !response.signal;
  const fail = (step, response) => {
    outcome.status = response?.signal ? 'interrupted' : 'failed';
    outcome.failedStep = step;
    result.status = 'notarization-failed';
    save();
    if (!response?.signal && outcome.submissionId && outcome.logStatus === undefined) {
      if (fetchLog()?.signal) outcome.status = 'interrupted';
      save();
    }
    const error = new Error(`macOS notarization ${outcome.status} during ${step}; see ${resultPath}`);
    error.result = result;
    throw error;
  };
  if (authentication === null) {
    outcome.status = 'skipped';
    save();
    return result;
  }
  result.status = 'notarization-in-progress';
  outcome.status = 'submitting';
  save();
  const submitted = invoke(['notarytool', 'submit', result.installer, '--wait', '--timeout', SUBMIT_WAIT, '--output-format', 'json', ...authentication], SUBMIT_TIMEOUT_MS);
  let submission;
  try { submission = JSON.parse(submitted?.stdout ?? ''); } catch { /* Invalid output cannot qualify the package. */ }
  if (UUID.test(submission?.id)) outcome.submissionId = submission.id;
  if (['Accepted', 'Invalid', 'Rejected', 'In Progress'].includes(submission?.status)) outcome.appleStatus = submission.status;
  if (!succeeded(submitted) || !outcome.submissionId || submission.status !== 'Accepted') fail('submit', submitted);
  outcome.notarized = true;
  const log = fetchLog();
  if (log?.signal) fail('log', log);
  outcome.status = 'stapling';
  save();
  const stapled = invoke(['stapler', 'staple', result.installer]);
  if (!succeeded(stapled)) fail('staple', stapled);
  outcome.stapled = true;
  outcome.status = 'validating';
  save();
  const validated = invoke(['stapler', 'validate', result.installer]);
  if (!succeeded(validated)) fail('validate', validated);
  outcome.validated = true;
  outcome.status = 'accepted';
  result.status = 'notarized-artifacts-built';
  save();
  return result;
}

/**
 * Checks the API key against Apple before a long build starts. Rejected or unreachable
 * credentials fail here instead of after compiling and signing.
 */
export function checkNotarizationCredentials(authentication, { run = spawnSync } = {}) {
  if (authentication === null) return;
  const response = run('/usr/bin/xcrun', ['notarytool', 'history', '--output-format', 'json', ...authentication], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: TOOL_TIMEOUT_MS });
  if (response?.status !== 0 || response.error || response.signal) {
    throw new Error('Apple rejected or could not check the notarization API key; fix notarization in the signing config or use --skip-notarization for a local test build');
  }
}

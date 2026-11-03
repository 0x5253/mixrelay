import { packageMetadata as packageMetadataPolicy, validatePackage as validatePackagePolicy, LINUX_PACKAGE_LIMITS, LINUX_EXECUTABLES as executables, VERSION_RECEIPT } from './core/linux-package-policy.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { loadContract } from './support-contract.mjs';
import { productContact } from './core/product-metadata-policy.mjs';

const { MAX_LISTING_BYTES, MAX_PACKAGE_CONTENT_BYTES, MAX_LISTING_ENTRIES, MAX_TAR_STREAM_BYTES, MAX_EXECUTABLE_BYTES, MAX_ELF_OUTPUT_BYTES, MAX_STDERR_BYTES, PROCESS_TIMEOUT_MS } = LINUX_PACKAGE_LIMITS;

const releaseDocuments = ["LICENSE.txt"];

const lifecycleHooks = {
  preinst: 'driver-before-change.sh', prerm: 'driver-pre-remove.sh',
  postinst: 'driver-after-install.sh', postrm: 'driver-after-remove.sh',
};
function inspectionEnvironment() {
  const env = { ...process.env, LC_ALL: 'C', LANG: 'C' };
  // GNU tar reads options from TAR_OPTIONS before explicit argv, including
  // checkpoint actions that can execute arbitrary commands. Listing uses a
  // separate environment that opts into numeric ownership explicitly.
  delete env.TAR_OPTIONS;
  return env;
}

function policyInputs() {
  const { minimums, metadata } = loadContract().contract;
  return { minimums, metadata: { homepage: metadata.homepage, contact: productContact(metadata) },
    releaseDocuments };
}
function packageMetadata(data) { return packageMetadataPolicy(data, policyInputs()); }
export function validatePackage(data) { return validatePackagePolicy(data, policyInputs()); }

function cappedText(stream) {
  let captured = '';
  let length = 0;
  stream.setEncoding('utf8');
  stream.on('data', chunk => {
    if (length < MAX_STDERR_BYTES) captured += chunk.slice(0, MAX_STDERR_BYTES - length);
    length += Buffer.byteLength(chunk);
  });
  return () => captured;
}

function streamPackageTar(archive, tarArguments, spawnProcess, limits) {
  return new Promise((resolvePromise, rejectPromise) => {
    let dpkg;
    let tar;
    try {
      dpkg = spawnProcess('dpkg-deb', ['--fsys-tarfile', archive], {
        env: inspectionEnvironment(),
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      tar = spawnProcess('tar', tarArguments, {
        env: inspectionEnvironment(),
        stdio: ['pipe', 'pipe', 'pipe'],
      });
    } catch (error) {
      dpkg?.kill('SIGKILL');
      tar?.kill('SIGKILL');
      rejectPromise(new Error(`Cannot start bounded Debian tar inspection: ${error.message}`));
      return;
    }
    const dpkgErrorText = cappedText(dpkg.stderr);
    const tarErrorText = cappedText(tar.stderr);
    const capturedOutput = Buffer.alloc(limits.captureBytes);
    let archiveBytes = 0;
    let outputBytes = 0;
    let capturedBytes = 0;
    let dpkgStatus;
    let tarStatus;
    let dpkgClosed = false;
    let tarClosed = false;
    let settled = false;
    let timer;

    const fail = error => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      dpkg.kill('SIGKILL');
      tar.kill('SIGKILL');
      rejectPromise(error);
    };
    const finishIfClosed = () => {
      if (settled || !dpkgClosed || !tarClosed) return;
      if (dpkgStatus !== 0) {
        fail(new Error(`dpkg-deb payload stream failed: ${dpkgErrorText() || `status ${dpkgStatus}`}`));
        return;
      }
      if (tarStatus !== 0) {
        fail(new Error(`tar package inspection failed: ${tarErrorText() || `status ${tarStatus}`}`));
        return;
      }
      if (limits.expectedOutputBytes !== undefined && outputBytes !== limits.expectedOutputBytes) {
        fail(new Error('Extracted executable byte count does not match Debian package metadata'));
        return;
      }
      settled = true;
      clearTimeout(timer);
      resolvePromise({ output: capturedOutput.subarray(0, capturedBytes), archiveBytes, outputBytes });
    };

    timer = setTimeout(() => fail(new Error('Bounded Debian tar inspection timed out')), PROCESS_TIMEOUT_MS);
    dpkg.on('error', error => fail(new Error(`Cannot start dpkg-deb payload stream: ${error.message}`)));
    tar.on('error', error => fail(new Error(`Cannot start tar package inspection: ${error.message}`)));
    dpkg.stdout.on('error', error => fail(new Error(`Cannot read dpkg-deb payload stream: ${error.message}`)));
    tar.stdout.on('error', error => fail(new Error(`Cannot read tar executable output: ${error.message}`)));
    dpkg.stdout.on('data', chunk => {
      archiveBytes += chunk.length;
      if (archiveBytes > MAX_TAR_STREAM_BYTES) fail(new Error('Debian uncompressed payload exceeds the bounded inspection size'));
    });
    tar.stdout.on('data', chunk => {
      const chunkStart = outputBytes;
      const chunkEnd = chunkStart + chunk.length;
      if (chunkEnd > limits.maxOutputBytes) {
        fail(new Error(limits.outputLimitMessage));
        return;
      }
      const capturedEnd = Math.min(capturedOutput.length, chunkEnd);
      if (chunkStart < capturedEnd) {
        chunk.copy(capturedOutput, chunkStart, 0, capturedEnd - chunkStart);
        capturedBytes = capturedEnd;
      }
      outputBytes = chunkEnd;
    });
    dpkg.on('close', (status, signal) => {
      dpkgStatus = status;
      dpkgClosed = true;
      if (signal && !settled) fail(new Error(`dpkg-deb payload stream stopped by ${signal}`));
      finishIfClosed();
    });
    tar.on('close', (status, signal) => {
      tarStatus = status;
      tarClosed = true;
      if (signal && !settled) fail(new Error(`tar package inspection stopped by ${signal}`));
      finishIfClosed();
    });
    tar.stdin.on('error', error => fail(new Error(`Cannot pipe Debian payload into tar: ${error.message}`)));
    dpkg.stdout.pipe(tar.stdin);
  });
}

function listNumericOwnedContents(archive, spawnProcess) {
  return streamPackageTar(archive, ['--numeric-owner', '-tvf', '-'], spawnProcess, {
    maxOutputBytes: MAX_LISTING_BYTES,
    captureBytes: MAX_LISTING_BYTES,
    outputLimitMessage: 'Debian payload listing exceeds the bounded inspection size',
  }).then(result => result.output.toString('utf8'));
}

async function extractOneElfHeader(archive, entry, spawnProcess) {
  const result = await streamPackageTar(
    archive,
    ['-xO', '-f', '-', '--', entry.memberPath],
    spawnProcess,
    {
      maxOutputBytes: entry.size,
      expectedOutputBytes: entry.size,
      captureBytes: entry.path === executables[0] ? entry.size : Math.min(entry.size, 64),
      outputLimitMessage: 'tar returned more executable bytes than package metadata declared',
    },
  );
  return { header: result.output, archiveBytes: result.archiveBytes };
}

async function extractElfHeaders(archive, executableEntries, spawnProcess = spawn) {
  const outputBytesExpected = executableEntries.reduce((total, entry) => total + entry.size, 0);
  if (outputBytesExpected > MAX_ELF_OUTPUT_BYTES) {
    throw new Error('Required executable payloads exceed the bounded extraction size');
  }
  const headers = {};
  let archiveBytesTotal = 0;
  // One member per tar invocation makes its stdout unambiguously belong to that exact path;
  // tar's archive-order output for multiple operands is not assumed to match operand order.
  for (const entry of executableEntries) {
    const result = await extractOneElfHeader(archive, entry, spawnProcess);
    archiveBytesTotal += result.archiveBytes;
    if (archiveBytesTotal > MAX_TAR_STREAM_BYTES * executableEntries.length) {
      throw new Error('Repeated Debian payload streams exceed the bounded inspection size');
    }
    headers[entry.path] = result.header;
  }
  return headers;
}

export async function inspectPackage(path, run = spawnSync, spawnProcess = spawn) {
  const archive = resolve(path);
  const field = name => {
    // Field names follow the archive for dpkg-deb --field.
    const result = run('dpkg-deb', ['--field', archive, name], {
      encoding: 'utf8', env: inspectionEnvironment(), timeout: 30_000, maxBuffer: 64 * 1024,
    });
    if (result.error || result.status !== 0 || result.signal) throw new Error(`Cannot read package ${name}`);
    return result.stdout;
  };
  const packageDetails = {
    packageName: field('Package').trim(),
    version: field('Version').trim(),
    homepage: field('Homepage'),
    maintainer: field('Maintainer'),
    architecture: field('Architecture'),
    depends: field('Depends'),
    contents: await listNumericOwnedContents(archive, spawnProcess),
  };
  for (const [script, source] of Object.entries(lifecycleHooks)) {
    const expectedHook = readFileSync(new URL(`../packaging/linux/${source}`, import.meta.url), 'utf8');
    const result = run('dpkg-deb', ['--info', archive, script], {
      encoding: 'utf8', env: inspectionEnvironment(), timeout: 30_000, maxBuffer: 64 * 1024,
    });
    if (result.error || result.status !== 0 || result.signal || result.stdout !== expectedHook) {
      throw new Error(`Missing or mismatched package lifecycle hook: ${script}`);
    }
  }
  const { architecture, executableEntries, files } = packageMetadata(packageDetails);
  const elfHeaders = await extractElfHeaders(archive, executableEntries, spawnProcess);
  const receipt = files.get(VERSION_RECEIPT);
  if (receipt.size > 64) throw new Error('Driver version receipt is oversized');
  const extracted = await extractOneElfHeader(archive, receipt, spawnProcess);
  packageDetails.versionReceipt = extracted.header.toString('utf8');
  return validatePackage({
    ...packageDetails,
    architecture,
    elfHeaders,
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 3) throw new Error('Usage: node scripts/check-linux-package.mjs PACKAGE.deb');
    console.log(JSON.stringify(await inspectPackage(process.argv[2]), null, 2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

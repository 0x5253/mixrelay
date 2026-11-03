import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { verifyMeasurement } from './macos-probe-verdict.mjs';
import { readProbePcmEvidence } from './macos-pcm-evidence.mjs';

export function macosProbeCommand(bundle, directory, uid) {
  const args = ['-W', '-n', '-g', '--stdout', join(directory, 'native-probe.stdout.json'),
    '--stderr', join(directory, 'native-probe.stderr.log'), bundle, '--args', '--json', '--timing',
    '--capture-file', join(directory, 'native-signal.f32'),
    '--hal-trace-file', join(directory, 'native-hal-io.json'), '--seconds', '3', '--channels', '2', uid];
  return { program: 'open', args, timeout: 60000 };
}

function readMeasurement(path, processFailure) {
  let measured;
  try { measured = JSON.parse(readFileSync(path, 'utf8')); }
  catch (error) {
    throw new Error('Native macOS probe measurement result could not be read', { cause: processFailure ?? error });
  }
  if (measured.error) {
    const { stage, osStatus, errno } = measured.error;
    throw new Error(`Native macOS probe failed: stage=${stage} osStatus=${osStatus} errno=${errno}`, { cause: processFailure });
  }
  if (processFailure) throw processFailure;
  return measured;
}

export async function macosSignal(device, directory, { execute, snapshot, schedulerTrace, trace, measurementBoundary }) {
  const uid = device.nativeDeviceIds.find(id => id.startsWith('io.mixrelay.audio.endpoint.'));
  assert.ok(uid, 'Missing canonical CoreAudio device identity');
  const bundle = execute('zsh', ['scripts/probe-macos-roundtrip.sh', '--prepare-bundle', 'io.mixrelay.qualification.probe'],
    { encoding: 'utf8', timeout: 60000 }).trim();
  const stdout = join(directory, 'native-probe.stdout.json');
  let measured, measurementFailure, product, hostRoundtrip;
  const errors = [];
  try {
    try {
      execute('open', ['-W', '-n', '-g', '--stdout', join(directory, 'native-probe-authorization.json'),
        '--stderr', join(directory, 'native-probe-authorization.stderr.log'), bundle,
        '--args', '--authorize-input', '--json'], { timeout: 60000 });
      const authorization = JSON.parse(readFileSync(join(directory, 'native-probe-authorization.json'), 'utf8'));
      assert.equal(authorization.inputAuthorization?.final, 3, 'Native probe consent must finish before observer suspension');
    } catch (error) { throw new Error(`Native macOS probe authorization failed: ${error.message}`, { cause: error }); }
    if (schedulerTrace) assert.equal(process.env.GITHUB_ACTIONS, 'true', 'Scheduler diagnosis requires a disposable hosted Mac');
    const command = macosProbeCommand(bundle, directory, uid);
    const measure = () => {
      let processFailure;
      try { execute(command.program, command.args, { timeout: command.timeout }); }
      catch (error) { processFailure = error; }
      measured = readMeasurement(stdout, processFailure);
      const evidence = readProbePcmEvidence(directory, measured);
      product = evidence.product;
      writeFileSync(join(directory, 'native-product-pcm.json'), JSON.stringify(evidence, null, 2) + '\n', { flag: 'wx' });
      try { hostRoundtrip = verifyMeasurement(measured, product); }
      catch (error) { measurementFailure = error.measurementFailure; throw error; }
    };
    await measurementBoundary(() => schedulerTrace ? trace(directory, measure) : measure());
  } catch (error) { errors.push(error); }
  finally {
    const executable = `${bundle}/Contents/MacOS/MRRoundTripProbe`;
    const pattern = `^${executable.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}( |$)`;
    try { execute('pkill', ['-TERM', '-f', pattern], { timeout: 5000 }); }
    catch (error) { if (error.status !== 1) errors.push(error); }
    if (measured) {
      try { snapshot(directory); }
      catch (error) { console.error(`Native post-signal process snapshot: ${error.message}`); }
    }
  }
  if (errors.length) {
    const failure = errors.length === 1 ? errors[0] : new AggregateError(errors, errors.map(error => error.message).join('; '));
    if (measurementFailure) failure.measurementFailure = measurementFailure;
    throw failure;
  }
  writeFileSync(join(directory, 'native-signal.json'), JSON.stringify({ ...measured, product, hostRoundtrip }, null, 2) + '\n', { flag: 'wx' });
}

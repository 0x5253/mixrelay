// Capture the physical reference and the OS-visible engine output, preserving
// AVFoundation timestamps. Never changes the user's graph or device defaults.
import { engineSocket, socketJson } from '../local-socket.mjs';
import { spawn } from 'node:child_process';
import { mkdir, open, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

if (!process.argv[2]) throw new Error('Usage: macos-microdropouts.mjs OUTPUT_DIRECTORY [SECONDS]');
const directory = resolve(process.argv[2]);
const seconds = Number(process.argv[3] ?? 30);
if (!Number.isFinite(seconds) || seconds < 1 || seconds > 120) throw new Error('Invalid duration');
await mkdir(directory, { recursive: false });
const observations = [];
const socket = engineSocket();
async function read(path) {
  return socketJson(socket, `/api/v1/${path}`, {
    headers: { host: 'mixrelay-engine' }, timeoutMs: 2000,
  });
}
async function observe() {
  const at = new Date().toISOString();
  try {
    const [status, graph] = await Promise.all([read('status'), read('graph')]);
    observations.push({ at, status, graph });
  } catch (error) {
    observations.push({ at, error: String(error) });
  }
}
await observe();
const trace = await open(`${directory}/capture.log`, 'wx');
const ffmpeg = spawn('/opt/homebrew/bin/ffmpeg', [
  '-hide_banner', '-nostats', '-loglevel', 'verbose', '-debug_ts', '-n', '-copyts',
  '-thread_queue_size', '1024', '-f', 'avfoundation', '-i', ':MacBook Pro 마이크',
  '-thread_queue_size', '1024', '-f', 'avfoundation', '-i', ':MixRelay IO',
  '-map', '0:a', '-map', '1:a', '-c:a', 'pcm_f32le', '-f', 'nut', `${directory}/capture.nut`,
], { stdio: ['pipe', 'ignore', trace.fd] });
const completion = new Promise((resolve, reject) => {
  ffmpeg.once('error', reject);
  ffmpeg.once('exit', (code, signal) => resolve({ code, signal }));
});
let inFlight;
const interval = setInterval(() => {
  if (!inFlight) inFlight = observe().finally(() => { inFlight = undefined; });
}, 250);
const stop = setTimeout(() => ffmpeg.stdin.end('q\n'), seconds * 1000);
// A blocked native open may never consume stdin. This affects only our child,
// not any other recorder, audio server or user application.
const forceStop = setTimeout(() => ffmpeg.kill('SIGKILL'), (seconds + 5) * 1000);
ffmpeg.stdin.on('error', () => {}); // Exit can close stdin before the stop request.
let result;
try {
  result = await completion;
} finally {
  clearInterval(interval);
  clearTimeout(stop);
  clearTimeout(forceStop);
  await inFlight;
  await observe();
  await trace.close();
  await writeFile(`${directory}/observations.json`, JSON.stringify({ result, observations }), { flag: 'wx' });
}
console.log(JSON.stringify({ directory, ...result, samples: observations.length }));
if (result.code !== 0) process.exitCode = 1;

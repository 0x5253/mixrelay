import { engineSocket, socketJson } from '../local-socket.mjs';
import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import { rawPcmArgs, parsePwDump } from "./pipewire-cli.mjs";

// Explicit guest-only smoke: route a quiet synthetic stereo signal exclusively
// through MixRelay's virtual devices. Never change the desktop's default sink.
assert.equal(process.platform, "linux");
assert.equal(process.argv[2], "--run", "Pass --run inside a disposable logged-in Linux guest");
assert.notEqual(process.getuid(), 0);
const inventory = parsePwDump(execFileSync("pw-dump", { encoding: "utf8" }));
const prefix = "io.mixrelay.default-virtual-io";
for (const [suffix, mediaClass] of [["render", "Audio/Sink"], ["capture", "Audio/Source"]]) {
  assert.ok(inventory.some(item => item.info?.props?.["node.name"] === `${prefix}.${suffix}`
    && item.info.props["media.class"] === mediaClass), `Missing explicit ${suffix} target`);
}
const socket = engineSocket();
const requestOptions = { headers: { host: 'mixrelay-engine' }, timeoutMs: 2000 };
const children = [];
function launch(args) {
  const child = spawn("pw-cat", args, { stdio: ["pipe", "pipe", "pipe"] });
  const state = { child, error: null, stderr: "", exited: false };
  child.on("error", error => { state.error = error; });
  child.on("exit", (code, signal) => { state.exited = true; state.code = code; state.signal = signal; });
  child.stderr.on("data", data => { state.stderr = (state.stderr + data).slice(-8192); });
  child.stdin.on("error", error => { state.error = error; });
  children.push(state);
  return state;
}
const common = rawPcmArgs(execFileSync("pw-cat", ["--help"], { encoding: "utf8", timeout: 5000 }));
let bytes = 0;
const chunks = [];
try {
  const recorder = launch(["--record", ...common, "--target", `${prefix}.capture`, "-"]);
  recorder.child.stdout.on("data", data => {
    bytes += data.length;
    if (bytes <= 4 * 1024 * 1024) chunks.push(data);
    else recorder.child.kill("SIGTERM"); // Bound memory even if a backend runs faster than real time.
  });
  await delay(300);
  const player = launch(["--playback", ...common, "--target", `${prefix}.render`, "-"]);
  const frames = 48000 * 4;
  const signal = Buffer.alloc(frames * 8);
  for (let frame = 0; frame < frames; frame++) {
    signal.writeFloatLE(0.125 * Math.sin(2 * Math.PI * 440 * frame / 48000), frame * 8);
    signal.writeFloatLE(0.0625 * Math.sin(2 * Math.PI * 880 * frame / 48000), frame * 8 + 4);
  }
  player.child.stdin.end(signal);
  const meters = [];
  for (let index = 0; index < 12; index++) {
    await delay(250);
    meters.push(await socketJson(socket, '/api/v1/meters', requestOptions));
  }
  for (const state of children) {
    assert.equal(state.error, null, state.error?.message);
    assert.equal(state.exited, false, `pw-cat exited prematurely: ${state.stderr}`);
  }
  const recorded = Buffer.concat(chunks);
  const peaks = [0, 0];
  let activeFrames = 0;
  let firstActive = -1;
  let lastActive = -1;
  let silentRun = 0;
  let longestInternalSilence = 0;
  let longestSilenceStartFrame = -1;
  for (let offset = 0; offset + 8 <= recorded.length; offset += 8) {
    const left = recorded.readFloatLE(offset);
    const right = recorded.readFloatLE(offset + 4);
    assert.ok(Number.isFinite(left) && Number.isFinite(right));
    peaks[0] = Math.max(peaks[0], Math.abs(left));
    peaks[1] = Math.max(peaks[1], Math.abs(right));
    if (Math.abs(left) > 0.001) {
      activeFrames++;
      if (firstActive < 0) firstActive = offset / 8;
      else if (silentRun > longestInternalSilence) {
        longestInternalSilence = silentRun;
        longestSilenceStartFrame = offset / 8 - silentRun;
      }
      lastActive = offset / 8;
      silentRun = 0;
    } else silentRun++;
  }
  const meterPeaks = ["from_os", "to_os"].map(port => {
    const peaks = [0, 0];
    for (const snapshot of meters) {
      const node = snapshot.nodes.find(node => node.nodeId === "default-virtual-io" && node.portId === port);
      node?.peaks.forEach((value, index) => { peaks[index] = Math.max(peaks[index] ?? 0, value); });
    }
    return { port, peaks };
  });
  const activeFraction = activeFrames / Math.max(1, lastActive - firstActive + 1);
  const runtimeStatus = await socketJson(socket, '/api/v1/status', requestOptions);
  console.log(JSON.stringify({ recordedFrames: Math.floor(recorded.length / 8), activeFrames, peaks, meterPeaks,
    activeFraction, longestInternalSilenceFrames: longestInternalSilence,
    firstActiveFrame: firstActive, longestSilenceStartFrame, runtimeStatus,
    notVerified: ["sample accuracy", "latency", "hardware", "multichannel", "endurance"] }, null, 2));
  assert.equal(runtimeStatus.audioSession?.processingState, "running", "DSP worker is not running");
  assert.ok(activeFrames > 48000, "Did not capture at least one second of active signal");
  assert.ok(peaks[0] > 0.11 && peaks[0] < 0.14, "Left channel amplitude mismatch");
  assert.ok(peaks[1] > 0.05 && peaks[1] < 0.075, "Right channel amplitude mismatch");
  assert.ok(meterPeaks.every(node => node.peaks[0] > 0.1 && node.peaks[1] > 0.05), "Input/output meters did not respond");
  // Ignore initial connection latency, but not holes between active samples.
  // A continuous 440 Hz sine only has isolated samples near zero, not silent blocks.
  assert.ok(activeFraction > 0.97 && longestInternalSilence < 48,
    "Continuous input developed silent gaps inside the recorded output");
} finally {
  for (const state of children) if (!state.exited) state.child.kill("SIGTERM");
  await delay(300);
  for (const state of children) if (!state.exited) state.child.kill("SIGKILL");
}

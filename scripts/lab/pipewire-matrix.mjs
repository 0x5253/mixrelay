import { engineSocket, socketJson, socketRequest } from '../local-socket.mjs';
import { engineOperations } from './engine-operations.mjs';
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { rawPcmArgs, parsePwDump, nodesOwnedByPid } from "./pipewire-cli.mjs";

// Run only in the owned transient engine harness, never against a user graph.
assert.equal(process.platform, "linux");
assert.equal(process.argv[2], "--run");
const pid = Number(process.env.MIXRELAY_LAB_OWNED_ENGINE_PID);
assert.ok(Number.isSafeInteger(pid) && pid > 1, "Use guest-native-matrix-check.sh");
const prefix = "io.mixrelay.lab-native";
const socket = engineSocket();
const { api: operationApi } = engineOperations({
  request: (path, options) => socketRequest(socket, path, options), now: Date.now, delay,
}, 'pipewire-matrix');
async function api(path, body) {
  if (body !== undefined) return operationApi(path, body);
  return socketJson(socket, `/api/v1/${path}`, {
    headers: { host: 'mixrelay-engine' }, timeoutMs: 5000,
  });
}
async function until(check, message) {
  for (let n = 0; n < 100; n++) { if (await check()) return; await delay(30); }
  assert.fail(message);
}
function inventory() { return parsePwDump(execFileSync("pw-dump", { encoding: "utf8", timeout: 5000 })); }
function ownNodes() { return nodesOwnedByPid(inventory(), pid); }
const initial = await api("graph");
assert.equal(initial.revision, 1);
assert.equal(initial.nodes.length, 1);
let revision = initial.revision;
async function apply(nodes, edges = []) {
  await api("graph", { schemaVersion: initial.schemaVersion, revision: ++revision, nodes, edges });
  await until(async () => (await api("graph/status")).activeRevision === revision, "Graph did not activate");
}
async function empty() {
  await apply([]);
  await until(() => ownNodes().every(o => o.info.props["node.name"] === "io.mixrelay.graph"), "Retired endpoint leaked");
  assert.equal((await api("status")).audioSession.processingState, "running", "Empty graph stopped its native clock");
}
function endpoint(input, output) {
  const stream = n => ({ channels: n, channelLayout: n === 2 ? "stereo" : "discrete" });
  return { id: "lab-native", type: "virtual_endpoint", config: {
    id: "lab-native", displayName: "MixRelay isolated channel test", render: input ? stream(input) : null,
    capture: output ? stream(output) : null, supportedSampleRates: [48000], defaultSampleRate: 48000 } };
}
function edge(source, destination, index) {
  return { id: `route-${index}`, source: { nodeId: "lab-native", portId: "from_os" },
    destination: { nodeId: "lab-native", portId: "to_os" },
    channelMapping: { mode: "explicit", routes: [{ sourceChannel: source, destinationChannel: destination }] } };
}
async function create(input, output, routes) {
  await empty();
  await apply([endpoint(input, output)], routes.map(([s, d], i) => edge(s, d, i)));
  await until(() => ownNodes().filter(o => o.info.props["node.name"].startsWith(prefix)).length ===
    Number(input > 0) + Number(output > 0), "Native endpoint did not register");
  await until(async () => (await api("status")).driverState === "ready", "Endpoint not observed ready");
}
const raw = rawPcmArgs(execFileSync("pw-cat", ["--help"], { encoding: "utf8" })).includes("--raw") ? ["--raw"] : [];
function args(channels) { return [...raw, "--rate", "48000", "--channels", String(channels), "--format", "f32",
  "--channel-map", channels === 2 ? "stereo" : Array.from({ length: channels }, (_, c) => `AUX${c}`).join(",")]; }
async function signalTest(input, output, routes, application = false) {
  await create(input, output, application ? [] : routes);
  const children = [];
  const launch = args => {
    const child = spawn("pw-cat", args, { stdio: ["pipe", "pipe", "pipe"] });
    const state = { child, error: null, stderr: "", exited: false };
    child.on("error", e => { state.error = e; });
    child.on("exit", () => { state.exited = true; });
    child.stdin.on("error", e => { state.error = e; });
    child.stderr.on("data", b => { state.stderr = (state.stderr + b).slice(-4096); });
    children.push(state); return state;
  };
  try {
    const recorder = launch(["--record", ...args(output), "--target", `${prefix}.capture`, "-"]);
    const chunks = []; let bytes = 0;
    recorder.child.stdout.on("data", b => { bytes += b.length;
      if (bytes < 48 * 1024 * 1024) chunks.push(b); else recorder.child.kill("SIGTERM"); });
    await delay(250);
    const player = launch(["--playback", ...args(input), "--target", `${prefix}.render`, "-"]);
    const frames = 48000 * 3;
    const signal = Buffer.alloc(frames * input * 4);
    for (let f = 0; f < frames; f++) for (let c = 0; c < input; c++)
      signal.writeFloatLE((c + 1) / 1024 * Math.sin(2 * Math.PI * 1000 * f / 48000), (f * input + c) * 4);
    player.child.stdin.end(signal);
    if (application) {
      await until(() => nodesOwnedByPid(inventory(), player.child.pid).some(o =>
        o.info.props["media.class"] === "Stream/Output/Audio"), "Synthetic application stream not registered");
      const app = { id: "lab-app", type: "application_capture", config: {
        selector: { match: "process_id", process_id: player.child.pid, include_descendants: false },
        stream: { channels: 2, channelLayout: "stereo" }, sourcePlayback: "preserve", gainDb: -6.0206 } };
      await apply([endpoint(input, output), app], routes.map(([s, d], i) => ({
        ...edge(s, d, i), source: { nodeId: "lab-app", portId: "audio" } })));
    }
    await delay(1500);
    for (const child of children) { assert.equal(child.error, null, child.error?.message); assert.equal(child.exited, false, child.stderr); }
    const recorded = Buffer.concat(chunks);
    const peaks = Array(output).fill(0), expected = Array(output).fill(0);
    for (const [s, d] of routes) expected[d] += (s + 1) / 1024 * (application ? 0.5 : 1);
    let active = 0, silent = 0, maximumGap = 0;
    for (let offset = 0; offset + output * 4 <= recorded.length; offset += output * 4) {
      let nonzero = false;
      for (let c = 0; c < output; c++) {
        const value = Math.abs(recorded.readFloatLE(offset + c * 4)); assert.ok(Number.isFinite(value));
        peaks[c] = Math.max(peaks[c], value); nonzero ||= value > 0.0001;
      }
      if (nonzero) { if (active) maximumGap = Math.max(maximumGap, silent); active++; silent = 0; } else silent++;
    }
    console.log(JSON.stringify({ input, output, routes, application, active, maximumGap, peaks,
      runtimeStatus: await api("status") }));
    assert.ok(active > 24000, `No sustained ${input} -> ${output} signal`);
    for (let c = 0; c < output; c++) assert.ok(Math.abs(peaks[c] - expected[c]) < 0.00002,
      `Channel ${c + 1}: expected ${expected[c]}, received ${peaks[c]}`);
    assert.ok(maximumGap < 48, `Internal silent gap ${maximumGap} frames`);
    console.log(`PASS: ${input} -> ${output} native channel routing`);
  } finally {
    for (const s of children) if (!s.exited) s.child.kill("SIGTERM");
    await delay(200);
    for (const s of children) if (!s.exited) s.child.kill("SIGKILL");
  }
}
for (const channels of [2, 8, 16, 32, 64])
  await signalTest(channels, channels, Array.from({ length: channels }, (_, c) => [c, channels - c - 1]));
await signalTest(64, 16, [[12, 0], [32, 15]]); // channel 13 and 33, explicitly not the first stereo pair
await signalTest(8, 2, [[0, 0], [2, 0], [1, 1], [3, 1]]); // native multi-source-channel summing
await signalTest(2, 2, [[0, 0], [1, 1]], true); // actual per-process capture, with input gain
for (const [input, output] of [[2, 0], [0, 2]]) await create(input, output, []);
await empty();
const rss = [];
for (let n = 0; n < 24; n++) {
  await create(64, 16, []); await empty();
  const status = readFileSync(`/proc/${pid}/status`, "utf8");
  rss.push(Number(status.match(/^VmRSS:\s+(\d+)/m)?.[1]));
}
const tail = rss.slice(8);
assert.ok(tail.every(Number.isFinite));
assert.ok(Math.max(...tail) - Math.min(...tail) < 8 * 1024, "Unbounded memory growth during endpoint teardown");
console.log(JSON.stringify({ teardownCycles: 24, rssKiB: rss, result: "passed",
  notVerified: ["long endurance", "hardware clock drift", "device loss recovery", "other OS"] }));

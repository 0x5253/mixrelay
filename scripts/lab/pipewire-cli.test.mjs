import assert from "node:assert/strict";
import test from "node:test";
import { rawPcmArgs, parsePwDump, nodesOwnedByPid } from "./pipewire-cli.mjs";

test("legacy pw-cat uses raw stdin/stdout without an unsupported flag", () => {
  assert.deepEqual(rawPcmArgs("--rate RATE\n--format FORMAT\n"),
    ["--rate", "48000", "--channels", "2", "--format", "f32", "--channel-map", "stereo"]);
});
test("modern pw-cat explicitly requests raw PCM", () => {
  assert.equal(rawPcmArgs("  -a, --raw   Raw PCM\n")[0], "--raw");
});
test("similarly named options do not imply raw PCM support", () => {
  assert.ok(!rawPcmArgs("--raw-other\n").includes("--raw"));
});

test("registry dumps merge concurrent arrays and remove retired objects", () => {
  assert.deepEqual(parsePwDump('[{"id":1,"type":"Node","info":{"props":{"name":"] ["}}},{"id":2}]\n' +
    '[{"id":1,"info":{"props":{"channels":64}}},{"id":2,"info":null}]'),
    [{id:1,type:"Node",info:{props:{name:"] [",channels:64}}}]);
});
test("malformed or incomplete registry JSON is not a successful empty snapshot", () => {
  for (const text of ['[{"id":1}', '[{"id":1}] trailing', '{}', ']', '[{"info":{}}]'])
    assert.throws(() => parsePwDump(text));
});

test('native node ownership follows client identity without matching unrelated processes', () => {
  const client = { id: 52, type: 'PipeWire:Interface:Client', info: { props: { 'application.process.id': 8459 } } };
  const stream = { id: 53, type: 'PipeWire:Interface:Node', info: { props: { 'client.id': 52 } } };
  const direct = { id: 54, type: 'PipeWire:Interface:Node', info: { props: { 'application.process.id': '8459' } } };
  const unrelated = { id: 55, type: 'PipeWire:Interface:Node', info: { props: { 'client.id': 999 } } };
  assert.deepEqual(nodesOwnedByPid([client, stream, direct, unrelated], 8459), [stream, direct]);
  assert.deepEqual(nodesOwnedByPid([client, stream, direct, unrelated], 8460), []);
  assert.throws(() => nodesOwnedByPid([], undefined));
});

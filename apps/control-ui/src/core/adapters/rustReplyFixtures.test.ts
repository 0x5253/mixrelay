import { readdirSync, readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { protocolErrorCodes } from '../domain/control/errors';
import {
  controlStateSnapshot,
  graphCatalog,
  mixRelayStatus,
  runtimeGraphDocument,
} from './control/protocolDecoders';
import { unnamedFields } from './decode';
import { TauriDriverPort } from './tauri/TauriDriverPort';
import { TauriUpdatesPort } from './tauri/TauriUpdatesPort';

/** Replies serialized by their Rust owners; `pnpm fixtures:replies` regenerates them. */
const directory = new URL('../../../../../tests/fixtures/control-replies/', import.meta.url);
const fixture = (name: string): unknown =>
  JSON.parse(readFileSync(new URL(`${name}.json`, directory), 'utf8'));
/** A native command that answers with `value`. */
const reply = (value: unknown) => async () => value;
const fixtureNames = () => readdirSync(directory).map((file) => file.replace(/\.json$/u, ''));
const variantSuffix = '.variants';

/** A copy of `value` with the field at a path such as `devices[0].state` replaced. */
function replaced(value: unknown, path: string, replacement: unknown): unknown {
  const copy = structuredClone(value);
  const keys = path.match(/[^.[\]]+/gu) ?? [];
  let parent = copy as Record<string, unknown>;
  for (const key of keys.slice(0, -1)) parent = parent[key] as Record<string, unknown>;
  const last = keys.at(-1) as string;
  if (!(last in parent)) throw new Error(`fixture has no field ${path}`);
  parent[last] = replacement;
  return copy;
}

const decoders: Record<string, (value: unknown) => Promise<unknown> | unknown> = {
  status: (value) => mixRelayStatus(value, 'status'),
  graph: (value) => runtimeGraphDocument(value, 'graph'),
  catalog: (value) => graphCatalog(value, 'catalog'),
  'telemetry-state': (value) => controlStateSnapshot(value, 'telemetry-state'),
  'driver-snapshot': (value) => new TauriDriverPort(reply(value)).getSnapshot(),
  updates: (value) => new TauriUpdatesPort(reply(value)).check(),
};

it('decodes every reply fixture without changing it', async () => {
  for (const [name, decode] of Object.entries(decoders)) {
    const value = fixture(name);
    await expect(Promise.resolve(decode(value)), name).resolves.toEqual(value);
  }
});

/** Fixtures that list contract values instead of holding a reply. */
const valueLists = ['error-codes'];

it('has a decoder for every reply fixture the Rust owners write', () => {
  const names = fixtureNames().filter(
    (name) => !name.endsWith(variantSuffix) && !valueLists.includes(name),
  );
  expect(names.sort()).toEqual(Object.keys(decoders).sort());
});

it('decodes every enum variant the Rust owners can send', async () => {
  const variantFiles = fixtureNames().filter((name) => name.endsWith(variantSuffix));
  expect(variantFiles.length).toBeGreaterThan(0);
  for (const file of variantFiles) {
    const name = file.slice(0, -variantSuffix.length);
    const decode = decoders[name];
    if (!decode) throw new Error(`${file} has no reply fixture decoder`);
    const byPath = fixture(file) as Record<string, unknown[]>;
    for (const [path, values] of Object.entries(byPath))
      for (const value of values)
        await expect(
          Promise.resolve().then(() => decode(replaced(fixture(name), path, value))),
          `${name} ${path} = ${JSON.stringify(value)}`,
        ).resolves.toBeDefined();
  }
});

/** Fields Rust sends that no UI code reads. Any other unnamed field means a contract field
 * was renamed or added on one side only. */
const ignoredFields = [
  'status.sampleFaults',
  'telemetry-state.status.sampleFaults',
  'telemetry-state.controls.enginePower',
  'telemetry-state.controls.graphs[].name',
  'telemetry-state.controls.graphs[].nodes[].recorder',
  'driver_status.ownerProcessId',
];

it('names every field the Rust owners send, apart from the ones the UI ignores', async () => {
  const unnamed = new Set<string>();
  for (const [name, decode] of Object.entries(decoders))
    for (const path of await unnamedFields(() => decode(fixture(name))))
      unnamed.add(path.replace(/\[\d+\]/gu, '[]'));
  expect([...unnamed].sort()).toEqual([...ignoredFields].sort());
});

it('interprets only refusal codes that the engine or the desktop sends', () => {
  // The desktop reports a failed or malformed engine exchange with this code itself.
  const sent = new Set([...(fixture('error-codes') as string[]), 'control_request_failed']);
  expect(protocolErrorCodes.filter((code) => !sent.has(code))).toEqual([]);
});

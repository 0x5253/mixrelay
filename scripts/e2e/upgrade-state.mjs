import assert from 'node:assert/strict';
import { existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, renameSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { digest } from '../support-contract.mjs';

export function upgradeGraph(document, graphId) {
  return {
    ...document,
    revision: document.revision + 1,
    expectedGraphId: graphId,
    nodes: [
      { id: 'upgrade-gain', type: 'gain', config: { channels: 2, gainDb: -6 } },
      { id: 'upgrade-recorder', type: 'recorder', config: { channels: 2 } },
    ],
    edges: [
      {
        id: 'upgrade-routing',
        source: { nodeId: 'upgrade-gain', portId: 'audio' },
        destination: { nodeId: 'upgrade-recorder', portId: 'audio' },
        channelMapping: { mode: 'auto' },
      },
    ],
  };
}

function authoredNodes(nodes) {
  return nodes
    .map(({ id, type, config, label = null, control = {} }) => {
      assert.ok(
        control && typeof control === 'object' && !Array.isArray(control),
        'Invalid saved node control',
      );
      return { id, type, config, label, control: { enabled: true, muted: false, ...control } };
    })
    .sort(byId);
}

const byId = (left, right) => left.id.localeCompare(right.id);

export function verifyAuthoredGraph(authored, saved) {
  assert.equal(saved.activeId, authored.expectedGraphId, 'Baseline changed the active graph');
  assert.deepEqual(
    authoredNodes(saved.document.nodes),
    authoredNodes(authored.nodes),
    'Baseline did not save authored nodes',
  );
  assert.deepEqual(
    [...saved.document.edges].sort(byId),
    [...authored.edges].sort(byId),
    'Baseline did not save authored routing',
  );
}

function driverSettings(snapshot) {
  assert.equal(snapshot.state, 'ready', 'Upgrade needs a ready Driver');
  assert.ok(
    !snapshot.persistenceError,
    `Driver settings were not saved: ${snapshot.persistenceError}`,
  );
  return structuredClone(
    snapshot.settings.devices.map((device) => {
      const formats = {};
      for (const direction of ['render', 'capture']) {
        if (!device.spec[direction]) continue;
        const current = device.nativeFormats[direction].current;
        assert.ok(current, `Missing current Driver ${direction} format`);
        formats[direction] = current;
      }
      assert.ok(device.observedSampleRate, 'Missing observed Driver sample rate');
      return { spec: device.spec, observedSampleRate: device.observedSampleRate, formats };
    }),
  );
}

async function mutateDriver(driver, action) {
  const snapshot = await driver();
  driverSettings(snapshot);
  const changed = await driver({ ...action, expectedRevision: snapshot.revision });
  driverSettings(changed);
  return changed;
}

export async function prepareUpgradeDriver(driver) {
  const snapshot = await driver();
  const defaults = driverSettings(snapshot);
  assert.equal(defaults.length, 1, 'Upgrade needs the free default device');
  const device = snapshot.settings.devices[0];
  const channels = [4, 8, 16, 32, 64].find(
    (value) =>
      snapshot.settings.renderChannels.includes(value) &&
      snapshot.settings.captureChannels.includes(value) &&
      value !== device.spec.render?.channels &&
      value !== device.spec.capture?.channels,
  );
  const sampleRate =
    device.supportedSampleRates.find(
      (value) => device.canSetSampleRate && value !== device.observedSampleRate,
    ) ?? device.observedSampleRate;
  // Linux currently advertises only the opened nominal rate. Channels and PCM
  // still change there; request another rate only when the Driver offers it.
  assert.ok(channels, 'Baseline must support nondefault free Driver channels');
  const spec = {
    ...device.spec,
    defaultSampleRate: sampleRate,
    render: { ...device.spec.render, channels, channelLayout: 'discrete' },
    capture: { ...device.spec.capture, channels, channelLayout: 'discrete' },
  };
  const configured = await mutateDriver(driver, { action: 'configure', spec });
  const current = configured.settings.devices[0];
  const sampleFormat = ['signed16', 'signed24', 'signed32'].find((value) =>
    ['render', 'capture'].every(
      (direction) =>
        value !== defaults[0].formats[direction]?.sampleFormat &&
        current.nativeFormats[direction].supported.some(
          (format) =>
            format.sampleRate === sampleRate &&
            format.channels === channels &&
            format.sampleFormat === value,
        ),
    ),
  );
  assert.ok(sampleFormat, 'Baseline must support a nondefault common PCM format');
  await mutateDriver(driver, { action: 'set_sample_format', endpointId: spec.id, sampleFormat });
  const saved = await driver();
  assert.deepEqual(
    driverSettings(saved),
    [
      {
        spec,
        observedSampleRate: sampleRate,
        formats: {
          render: { sampleRate, channels, sampleFormat },
          capture: { sampleRate, channels, sampleFormat },
        },
      },
    ],
    'Baseline did not save the requested Driver settings',
  );
  return { defaults, snapshot: saved };
}

export async function restoreUpgradeDriver(driver, defaults) {
  for (const device of defaults) {
    await mutateDriver(driver, { action: 'configure', spec: device.spec });
    for (const [direction, format] of Object.entries(device.formats)) {
      await mutateDriver(driver, {
        action: 'set_format',
        endpointId: device.spec.id,
        direction,
        format,
      });
    }
  }
  assert.deepEqual(driverSettings(await driver()), defaults, 'Driver defaults were not restored');
}

export function preservedState(catalog, driver, path, defaultDevices) {
  assert.ok(
    typeof catalog.activeId === 'string' && catalog.activeId.length > 0,
    'Upgrade needs an active graph',
  );
  assert.ok(catalog.document.nodes.length > 0, 'Upgrade needs an authored graph');
  assert.ok(catalog.document.edges.length > 0, 'Upgrade needs authored routing');
  assert.ok(driver.settings.devices.length > 0, 'Upgrade needs saved Driver devices');
  const devices = driverSettings(driver);
  assert.ok(defaultDevices?.length > 0, 'Upgrade needs the original Driver defaults');
  assert.notDeepEqual(devices, defaultDevices, 'Upgrade needs nondefault Driver settings');
  return structuredClone({
    activeId: catalog.activeId,
    nodes: catalog.document.nodes,
    edges: catalog.document.edges,
    devices,
    defaultDevices,
    path,
    sha256: digest(readFileSync(path)),
  });
}

export function verifyPreservedState(before, catalog, driver) {
  assert.deepEqual(
    {
      activeId: catalog.activeId,
      nodes: authoredNodes(catalog.document.nodes),
      edges: [...catalog.document.edges].sort(byId),
    },
    {
      activeId: before.activeId,
      nodes: authoredNodes(before.nodes),
      edges: [...before.edges].sort(byId),
    },
    'Upgrade lost the saved graph',
  );
  assert.deepEqual(driverSettings(driver), before.devices, 'Upgrade lost Driver settings');
  assert.equal(
    digest(readFileSync(before.path)),
    before.sha256,
    'Upgrade changed the saved session file',
  );
}

export async function saveWorkspace(api, operation, active, action, path) {
  const current = await api('graphs');
  await active(current.document.revision);
  const reply = await operation(
    'POST',
    '/api/v1/workspace',
    JSON.stringify({
      expectedRevision: current.document.revision,
      expectedCatalogRevision: current.catalogRevision,
      operation: { action, path },
    }),
  );
  assert.equal(reply.status, 200, reply.body);
  await active(reply.json.document.revision);
  return reply.json;
}

export function isolateFreshState(
  os,
  output,
  { home = homedir(), environment = process.env, disposable = false } = {},
) {
  assert.equal(disposable, true, 'State isolation requires a disposable QA session');
  const root = realpathSync(home);
  const config = environment.XDG_CONFIG_HOME || join(root, '.config');
  const data = environment.XDG_DATA_HOME || join(root, '.local/share');
  const paths =
    os === 'macos'
      ? ['MixRelay', 'io.mixrelay.driver', 'io.mixrelay.desktop'].map((name) =>
          join(root, 'Library/Application Support', name),
        )
      : [
          join(config, 'mixrelay'),
          join(config, 'mixrelay-driver'),
          join(config, 'io.mixrelay.desktop'),
          join(data, 'io.mixrelay.desktop'),
        ];
  const normalized = paths.map((path) => resolve(path));
  for (const path of normalized) {
    assert.ok(path.startsWith(`${root}/`), 'QA state must stay inside the session home');
    assert.ok(
      resolve(output) !== path && !resolve(output).startsWith(`${path}/`),
      'Evidence must be outside isolated user state',
    );
    for (let current = path; current !== root; current = dirname(current)) {
      let entry;
      try {
        entry = lstatSync(current);
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
      }
      assert.ok(!entry?.isSymbolicLink(), `Symlink in QA state path: ${current}`);
    }
  }
  const backup = join(output, 'fresh-state');
  mkdirSync(backup);
  return normalized
    .filter((path) => existsSync(path))
    .map((path, index) => {
      const destination = join(backup, String(index));
      renameSync(path, destination);
      return { path: relative(root, path), backup: relative(output, destination) };
    });
}

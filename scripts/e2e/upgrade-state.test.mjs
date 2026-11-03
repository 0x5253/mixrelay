import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import {
  prepareUpgradeDriver,
  preservedState,
  restoreUpgradeDriver,
  saveWorkspace,
  upgradeGraph,
  verifyAuthoredGraph,
  verifyPreservedState,
} from './upgrade-state.mjs';

// Extracted from run 40019418055, debian-13-x64/upgrade/upgrade-session.mix.
const savedSession = JSON.parse(
  readFileSync(new URL('./fixtures/upgrade-session.json', import.meta.url)),
);

function savedCatalog() {
  const catalog = structuredClone(savedSession.catalog);
  return {
    ...catalog,
    document: catalog.graphs.find((graph) => graph.id === catalog.activeId).document,
  };
}

test('baseline save accepts serialized label and control defaults from the real session', () => {
  const saved = savedCatalog();
  const authored = upgradeGraph({ revision: 1 }, 'graph-1');
  assert.notDeepEqual(saved.document.nodes, authored.nodes);
  verifyAuthoredGraph(authored, saved);
  saved.document.nodes.reverse();
  verifyAuthoredGraph(authored, saved);
});

for (const [name, mutate] of [
  ['missing node', (s) => s.document.nodes.pop()],
  ['extra node', (s) => s.document.nodes.push({ ...s.document.nodes[0], id: 'extra' })],
  ['changed node ID', (s) => { s.document.nodes[0].id = 'different'; }],
  ['changed node type', (s) => { s.document.nodes[0].type = 'compressor'; }],
  ['missing config value', (s) => { delete s.document.nodes[0].config.gainDb; }],
  ['changed config value', (s) => { s.document.nodes[0].config.gainDb = 0; }],
  ['changed channels', (s) => { s.document.nodes[1].config.channels = 1; }],
  ['changed label', (s) => { s.document.nodes[0].label = 'different'; }],
  ['disabled node', (s) => { s.document.nodes[0].control.enabled = false; }],
  ['muted node', (s) => { s.document.nodes[0].control.muted = true; }],
  ['invalid null control', (s) => { s.document.nodes[0].control = null; }],
  ['invalid array control', (s) => { s.document.nodes[0].control = []; }],
  ['missing edge', (s) => { s.document.edges = []; }],
  ['extra edge', (s) => { s.document.edges.push({ ...s.document.edges[0], id: 'extra' }); }],
  ['changed edge ID', (s) => { s.document.edges[0].id = 'different'; }],
  ['changed edge destination', (s) => { s.document.edges[0].destination.nodeId = 'upgrade-gain'; }],
  ['changed edge port', (s) => { s.document.edges[0].source.portId = 'different'; }],
  ['changed channel mapping', (s) => { s.document.edges[0].channelMapping = { mode: 'manual', routes: [] }; }],
  ['changed active graph', (s) => { s.activeId = 'other'; }],
]) {
  test(`baseline save and upgrade preservation reject ${name}`, (t) => {
    const f = fixture(t);
    const before = preservedState(f.catalog, f.driver, f.path, f.defaults);
    const saved = savedCatalog();
    mutate(saved);
    assert.throws(() => verifyAuthoredGraph(upgradeGraph({ revision: 1 }, 'graph-1'), saved));
    assert.throws(() => verifyPreservedState(before, saved, f.driver));
  });
}

function defaultDriver() {
  const format = { sampleRate: 48000, channels: 2, sampleFormat: 'float32' };
  return {
    state: 'ready',
    revision: 1,
    settings: {
      canCreate: false,
      canRename: false,
      renderChannels: [2, 4],
      captureChannels: [2, 4],
      devices: [
        {
          spec: {
            id: '17e59d73-42c1-4f11-a8b4-6c2d913357e0',
            displayName: 'MixRelay IO',
            render: { channels: 2, channelLayout: 'stereo' },
            capture: { channels: 2, channelLayout: 'stereo' },
            defaultSampleRate: 48000,
            supportedSampleRates: [44100, 48000],
          },
          nativeDeviceIds: ['old-native-id'],
          observedSampleRate: 48000,
          canSetSampleRate: true,
          supportedSampleRates: [44100, 48000],
          nativeFormats: {
            render: { current: { ...format }, supported: [format] },
            capture: { current: { ...format }, supported: [format] },
          },
        },
      ],
    },
  };
}

function defaultDevices() {
  const device = defaultDriver().settings.devices[0];
  return [
    {
      spec: device.spec,
      observedSampleRate: 48000,
      formats: {
        render: device.nativeFormats.render.current,
        capture: device.nativeFormats.capture.current,
      },
    },
  ];
}

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'mixrelay-upgrade-state-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = join(directory, 'session.mix');
  const catalog = savedCatalog();
  writeFileSync(path, JSON.stringify(savedSession));
  const driver = defaultDriver();
  for (const direction of ['render', 'capture']) {
    driver.settings.devices[0].nativeFormats[direction].current.sampleFormat = 'signed16';
  }
  return { path, catalog, driver, defaults: defaultDevices() };
}

test('preservation compares authored content and current formats while ignoring runtime identity changes', (t) => {
  const f = fixture(t);
  const before = preservedState(f.catalog, f.driver, f.path, f.defaults);
  f.catalog.document.revision = 22;
  delete f.catalog.document.nodes[0].label;
  delete f.catalog.document.nodes[0].control;
  f.driver.revision = 42;
  f.driver.settings.devices[0].nativeDeviceIds = ['new-native-id'];
  f.driver.settings.devices[0].nativeFormats.render.supported = [];
  verifyPreservedState(before, f.catalog, f.driver);
});

for (const [name, mutate, expected] of [
  [
    'graph selection reset',
    (f) => {
      f.catalog.activeId = 'default-graph';
    },
    /saved graph/,
  ],
  [
    'graph document missing or reset',
    (f) => {
      f.catalog.document.nodes = [];
      f.catalog.document.edges = [];
    },
    /saved graph/,
  ],
  [
    'processor setting reset',
    (f) => {
      f.catalog.document.nodes[0].config.gainDb = 0;
    },
    /saved graph/,
  ],
  [
    'routing reset',
    (f) => {
      f.catalog.document.edges = [];
    },
    /saved graph/,
  ],
  [
    'Driver settings reset to the fixed-ID default device',
    (f) => {
      f.driver = defaultDriver();
    },
    /Driver settings/,
  ],
  [
    'Driver render PCM reset with the spec intact',
    (f) => {
      f.driver.settings.devices[0].nativeFormats.render.current.sampleFormat = 'float32';
    },
    /Driver settings/,
  ],
  [
    'Driver capture PCM reset with the spec intact',
    (f) => {
      f.driver.settings.devices[0].nativeFormats.capture.current.sampleFormat = 'float32';
    },
    /Driver settings/,
  ],
  [
    'Driver current sample rate reset with the spec intact',
    (f) => {
      f.driver.settings.devices[0].nativeFormats.render.current.sampleRate = 44100;
    },
    /Driver settings/,
  ],
  [
    'Driver current channels reset with the spec intact',
    (f) => {
      f.driver.settings.devices[0].nativeFormats.capture.current.channels = 4;
    },
    /Driver settings/,
  ],
  [
    'Driver observed sample rate reset',
    (f) => {
      f.driver.settings.devices[0].observedSampleRate = 44100;
    },
    /Driver settings/,
  ],
  [
    'Driver device configuration changed',
    (f) => {
      f.driver.settings.devices[0].spec.capture.channels = 4;
    },
    /Driver settings/,
  ],
  [
    'Driver device missing',
    (f) => {
      f.driver.settings.devices = [];
    },
    /Driver settings/,
  ],
  [
    'Driver native readback missing',
    (f) => {
      f.driver.settings.devices[0].nativeFormats.render.current = null;
    },
    /Missing current Driver/,
  ],
  [
    'Driver persistence failed',
    (f) => {
      f.driver.persistenceError = 'disk full';
    },
    /disk full/,
  ],
  [
    'session document reset',
    (f) => {
      writeFileSync(f.path, JSON.stringify({ nodes: [], edges: [] }));
    },
    /saved session file/,
  ],
  [
    'session document deleted',
    (f) => {
      rmSync(f.path);
    },
    /ENOENT/,
  ],
]) {
  test(`upgrade checker rejects ${name}`, (t) => {
    const f = fixture(t);
    const before = preservedState(f.catalog, f.driver, f.path, f.defaults);
    mutate(f);
    assert.throws(() => verifyPreservedState(before, f.catalog, f.driver), expected);
  });
}

test('a missing Driver settings file recreating the same default spec fails preservation', (t) => {
  const f = fixture(t);
  const settingsPath = join(f.path, '..', 'driver-settings.json');
  writeFileSync(settingsPath, JSON.stringify(f.driver));
  const restart = () =>
    existsSync(settingsPath) ? JSON.parse(readFileSync(settingsPath, 'utf8')) : defaultDriver();
  const before = preservedState(f.catalog, restart(), f.path, f.defaults);
  rmSync(settingsPath);
  const after = restart();
  assert.deepEqual(after.settings.devices[0].spec, f.driver.settings.devices[0].spec);
  verifyPreservedState(before, f.catalog, f.driver);
  assert.throws(() => verifyPreservedState(before, f.catalog, after), /Driver settings/);
});

test('default-only graph or Driver state cannot count as upgrade preservation evidence', (t) => {
  const f = fixture(t);
  assert.throws(
    () => preservedState(f.catalog, defaultDriver(), f.path, f.defaults),
    /nondefault Driver/,
  );
  f.catalog.document.edges = [];
  assert.throws(() => preservedState(f.catalog, f.driver, f.path, f.defaults), /authored routing/);
  f.catalog.document.nodes = [];
  assert.throws(() => preservedState(f.catalog, f.driver, f.path, f.defaults), /authored graph/);
});

function driverApi({
  persistenceError,
  ignorePcm = false,
  reject = false,
  rates = [44100, 48000],
  pcm = ['float32', 'signed16'],
  ignoreRestore = false,
} = {}) {
  let snapshot = defaultDriver();
  snapshot.settings.devices[0].supportedSampleRates = rates;
  const commands = [];
  const driver = async (command) => {
    if (!command) return structuredClone(snapshot);
    commands.push(structuredClone(command));
    assert.equal(command.expectedRevision, snapshot.revision);
    if (reject) throw new Error('Driver HTTP 409: revision mismatch');
    const device = snapshot.settings.devices[0];
    if (command.action === 'configure') {
      device.spec = structuredClone(command.spec);
      device.observedSampleRate = command.spec.defaultSampleRate;
      for (const direction of ['render', 'capture']) {
        const format = {
          sampleRate: device.observedSampleRate,
          channels: device.spec[direction].channels,
          sampleFormat: device.nativeFormats[direction].current.sampleFormat,
        };
        device.nativeFormats[direction] = {
          current: format,
          supported: pcm.map((sampleFormat) => ({ ...format, sampleFormat })),
        };
      }
    } else if (command.action === 'set_sample_format') {
      assert.equal(command.endpointId, device.spec.id);
      if (!ignorePcm)
        for (const direction of ['render', 'capture']) {
          device.nativeFormats[direction].current.sampleFormat = command.sampleFormat;
        }
    } else if (command.action === 'set_format') {
      assert.equal(command.endpointId, device.spec.id);
      if (!ignoreRestore)
        device.nativeFormats[command.direction].current = structuredClone(command.format);
    } else assert.fail(`Unexpected Driver action: ${command.action}`);
    snapshot.revision++;
    snapshot.persistenceError = persistenceError;
    return structuredClone(snapshot);
  };
  return { driver, commands };
}

test('baseline changes free Driver settings through revision-checked commands and restores defaults before audio checks', async () => {
  const { driver, commands } = driverApi();
  const prepared = await prepareUpgradeDriver(driver);
  assert.deepEqual(prepared.defaults, defaultDevices());
  const device = prepared.snapshot.settings.devices[0];
  assert.equal(device.spec.displayName, 'MixRelay IO');
  assert.equal(device.spec.render.channels, 4);
  assert.equal(device.spec.capture.channels, 4);
  assert.equal(device.spec.defaultSampleRate, 44100);
  assert.deepEqual(device.nativeFormats.render.current, {
    sampleRate: 44100,
    channels: 4,
    sampleFormat: 'signed16',
  });
  assert.deepEqual(device.nativeFormats.capture.current, {
    sampleRate: 44100,
    channels: 4,
    sampleFormat: 'signed16',
  });
  assert.deepEqual(
    commands.map((command) => command.action),
    ['configure', 'set_sample_format'],
  );
  assert.deepEqual(
    commands.map((command) => command.expectedRevision),
    [1, 2],
  );
  await restoreUpgradeDriver(driver, prepared.defaults);
  const restored = await driver();
  assert.deepEqual(restored.settings.devices[0].spec, defaultDriver().settings.devices[0].spec);
  assert.equal(restored.settings.devices[0].observedSampleRate, 48000);
  for (const direction of ['render', 'capture']) {
    assert.deepEqual(restored.settings.devices[0].nativeFormats[direction].current, {
      sampleRate: 48000,
      channels: 2,
      sampleFormat: 'float32',
    });
  }
  assert.deepEqual(
    commands.map((command) => command.action),
    ['configure', 'set_sample_format', 'configure', 'set_format', 'set_format'],
  );
});

test('baseline with Linux current-rate-only capabilities still saves nondefault channels and PCM', async () => {
  const { driver } = driverApi({ rates: [48000] });
  const prepared = await prepareUpgradeDriver(driver);
  const device = prepared.snapshot.settings.devices[0];
  assert.equal(device.spec.defaultSampleRate, 48000);
  for (const direction of ['render', 'capture']) {
    assert.deepEqual(device.nativeFormats[direction].current, {
      sampleRate: 48000,
      channels: 4,
      sampleFormat: 'signed16',
    });
  }
  await restoreUpgradeDriver(driver, prepared.defaults);
});

test('failed default restoration blocks the full audio suite', async () => {
  const { driver } = driverApi({ ignoreRestore: true });
  const prepared = await prepareUpgradeDriver(driver);
  await assert.rejects(
    restoreUpgradeDriver(driver, prepared.defaults),
    /defaults were not restored/,
  );
});

for (const [name, options, expected] of [
  ['native PCM readback stays at default', { ignorePcm: true }, /did not save/],
  ['native change succeeds but persistence fails', { persistenceError: 'disk full' }, /disk full/],
  ['Driver rejects the mutation', { reject: true }, /revision mismatch/],
  ['only default PCM is supported', { pcm: ['float32'] }, /nondefault common PCM/],
]) {
  test(`baseline seeding fails when ${name}`, async () => {
    await assert.rejects(prepareUpgradeDriver(driverApi(options).driver), expected);
  });
}

test('workspace save/open supplies current concurrency revisions and waits for the activated reply', async () => {
  const activated = [],
    operations = [];
  const api = async (route) => {
    assert.equal(route, 'graphs');
    return { document: { revision: 9 }, catalogRevision: 12 };
  };
  const operation = async (...args) => {
    assert.deepEqual(activated, [9], 'workspace save must wait for the submitted graph');
    operations.push(args);
    return { status: 200, json: { document: { revision: 10 } } };
  };
  await saveWorkspace(
    api,
    operation,
    async (revision) => activated.push(revision),
    'save',
    '/session.mix',
  );
  assert.deepEqual(operations, [
    [
      'POST',
      '/api/v1/workspace',
      JSON.stringify({
        expectedRevision: 9,
        expectedCatalogRevision: 12,
        operation: { action: 'save', path: '/session.mix' },
      }),
    ],
  ]);
  assert.deepEqual(activated, [9, 10]);
  await assert.rejects(
    saveWorkspace(
      api,
      async () => ({ status: 409, body: 'conflict' }),
      async (revision) => assert.equal(revision, 9, 'rejection cannot activate a new revision'),
      'open',
      '/session.mix',
    ),
    /conflict/,
  );
});

test('fresh state is archived before baseline initialization while external documents stay in place', async (t) => {
  const { existsSync, mkdirSync, readFileSync, realpathSync } = await import('node:fs');
  const { isolateFreshState } = await import('./upgrade-state.mjs');
  for (const os of ['macos', 'linux']) {
    const root = mkdtempSync(join(tmpdir(), 'mixrelay-isolate-state-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const home = realpathSync(root),
      output = join(home, 'evidence');
    mkdirSync(output);
    const directories =
      os === 'macos'
        ? [
            'Library/Application Support/MixRelay',
            'Library/Application Support/io.mixrelay.driver',
            'Library/Application Support/io.mixrelay.desktop',
          ]
        : [
            '.config/mixrelay',
            '.config/mixrelay-driver',
            '.config/io.mixrelay.desktop',
            '.local/share/io.mixrelay.desktop',
          ];
    for (const path of directories) {
      mkdirSync(join(home, path), { recursive: true });
      writeFileSync(join(home, path, 'state.json'), 'fresh candidate state');
    }
    writeFileSync(join(home, 'project.mix'), 'saved user document');
    const archived = isolateFreshState(os, output, { home, environment: {}, disposable: true });
    assert.equal(archived.length, directories.length);
    for (const item of archived) {
      assert.equal(existsSync(join(home, item.path)), false);
      assert.equal(
        readFileSync(join(output, item.backup, 'state.json'), 'utf8'),
        'fresh candidate state',
      );
    }
    assert.equal(readFileSync(join(home, 'project.mix'), 'utf8'), 'saved user document');
  }
});

test('state isolation rejects unacknowledged hosts, external paths and live or dangling symlinks before moving anything', async (t) => {
  const { mkdirSync, realpathSync, symlinkSync, existsSync } = await import('node:fs');
  const { isolateFreshState } = await import('./upgrade-state.mjs');
  const root = mkdtempSync(join(tmpdir(), 'mixrelay-isolate-guard-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const home = realpathSync(root),
    output = join(home, 'evidence');
  mkdirSync(output);
  const state = join(home, '.config/mixrelay');
  mkdirSync(state, { recursive: true });
  writeFileSync(join(state, 'graphs.json'), 'keep');
  assert.throws(() => isolateFreshState('linux', output, { home, environment: {} }), /disposable/);
  assert.throws(
    () =>
      isolateFreshState('linux', output, {
        home,
        environment: { XDG_CONFIG_HOME: '/etc' },
        disposable: true,
      }),
    /inside the session home/,
  );
  symlinkSync(join(home, 'missing-target'), join(home, '.config/mixrelay-driver'));
  assert.throws(
    () => isolateFreshState('linux', output, { home, environment: {}, disposable: true }),
    /Symlink/,
  );
  assert.equal(existsSync(join(state, 'graphs.json')), true);
  assert.equal(existsSync(join(output, 'fresh-state')), false);
});

test('baseline state contains a nondefault processor parameter and routing without needing a native audio source', async () => {
  const { upgradeGraph } = await import('./upgrade-state.mjs');
  const original = { schemaVersion: 2, revision: 8, nodes: [], edges: [] };
  const document = upgradeGraph(original, 'baseline-graph');
  assert.equal(document.expectedGraphId, 'baseline-graph');
  assert.equal(document.revision, 9);
  assert.deepEqual(original.nodes, []);
  assert.deepEqual(
    document.nodes.map((node) => node.type),
    ['gain', 'recorder'],
  );
  assert.equal(document.nodes[0].config.gainDb, -6);
  assert.equal(document.nodes[0].config.channels, document.nodes[1].config.channels);
  assert.deepEqual(document.edges[0].source, { nodeId: document.nodes[0].id, portId: 'audio' });
  assert.deepEqual(document.edges[0].destination, {
    nodeId: document.nodes[1].id,
    portId: 'audio',
  });
});

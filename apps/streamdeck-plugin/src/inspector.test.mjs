import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

test('fixed controls offer only valid choices and retain defaults; targets require selection', async () => {
  const elements = new Map();
  const document = {
    documentElement: {},
    querySelectorAll: () => [],
    getElementById(id) {
      if (!elements.has(id))
        elements.set(id, {
          options: [],
          replaceChildren() {
            this.options = [];
          },
          add(option) {
            this.options.push(option);
          },
        });
      return elements.get(id);
    },
  };
  const window = {};
  const sent = [];
  let socket;
  runInNewContext(await readFile('dist/com.rawsector.mixrelay.sdPlugin/ui/inspector.js', 'utf8'), {
    window,
    document,
    Option: class {
      constructor(label, value) {
        this.label = label;
        this.value = value;
      }
    },
    WebSocket: class {
      constructor() { socket = this; }
      send(value) { sent.push(JSON.parse(value)); }
    },
  });
  window.connectElgatoStreamDeckSocket(
    1234,
    'context',
    'register',
    JSON.stringify({ application: { language: 'ko' } }),
    JSON.stringify({ action: 'com.rawsector.mixrelay.recording', payload: { settings: {} } }),
  );
  assert.deepEqual(
    Array.from(elements.get('operation').options, (item) => item.value),
    ['record', 'pause', 'stop'],
  );
  assert.equal(elements.get('operation').options[0].label, '녹음');
  assert.equal(elements.get('operation').value, 'record');
  assert.equal(elements.get('direction').options.length, 2);
  assert.equal(elements.get('step').options.length, 4);
  assert.equal(elements.get('graph').options[0].value, '');
  assert.equal(elements.get('node').options[0].value, '');
  socket.onmessage({ data: JSON.stringify({ event: 'sendToPropertyInspector', payload: {
    type: 'snapshot', snapshot: { graphs: [{ id: 'graph', name: 'Studio', nodes: [{ nodeId: 'recorder',
      label: 'Recorder', capabilities: { recorder: true }, control: { enabled: true, muted: false } }] }] },
  } }) });
  elements.get('graph').value = 'graph';
  elements.get('graph').onchange();
  elements.get('node').value = 'recorder';
  elements.get('node').onchange();
  elements.get('operation').value = 'pause';
  elements.get('operation').onchange();
  assert.equal(sent.filter(message => message.event === 'setSettings').length, 3);
  assert.equal(sent.at(-1).payload.operation, 'pause');
  assert.equal(sent.at(-1).payload.nodeId, 'recorder');
  const html = await readFile('ui/index.html', 'utf8');
  assert.doesNotMatch(html, /<button|\bSave\b/);
});

import { volumeSteps } from '../src/action-settings.mjs';
import { InspectorModel } from '../src/inspector-model.mjs';
import { translate } from '../src/localization.mjs';

let socket;
let context;
let actionId;
let language = 'en';
let model;
const element = (id) => document.getElementById(id);
const t = (key) => translate(language, key);

function choices(id, items, selected) {
  const select = element(id);
  select.replaceChildren();
  const allowsEmpty = id === 'graph' || id === 'node';
  if (allowsEmpty)
    select.add(
      new Option(
        t(
          selected && !items.some(([value]) => value === selected)
            ? 'Unavailable — choose another'
            : 'Choose…',
        ),
        '',
      ),
    );
  for (const [value, label] of items) select.add(new Option(label, value));
  select.value = items.some(([value]) => value === selected)
    ? selected
    : allowsEmpty
      ? ''
      : (items[0]?.[0] ?? '');
  select.disabled = !model.snapshot || items.length === 0;
}

function render() {
  const { settings, snapshot, dial } = model;
  const { graphs, nodes, volume, recording, selected, summary, hint } = model.view();
  for (const label of document.querySelectorAll('[data-i18n]'))
    label.textContent = t(label.dataset.i18n);
  element('status').textContent = t(
    snapshot ? 'Connected to MixRelay' : 'Open MixRelay to select a node.',
  );
  element('status').hidden = !!snapshot;
  choices(
    'graph',
    graphs.map((graph) => [graph.id, graph.name]),
    settings.graphId,
  );
  choices(
    'node',
    nodes.map((node) => [node.nodeId, node.label || node.nodeId]),
    settings.nodeId,
  );
  choices(
    'operation',
    [
      ['record', t('Record')],
      ['pause', t('Pause')],
      ['stop', t('Stop')],
    ],
    selected.operation,
  );
  choices(
    'direction',
    [
      ['up', t('Increase')],
      ['down', t('Decrease')],
    ],
    settings.direction ?? 'up',
  );
  choices(
    'step',
    volumeSteps.map((step) => [String(step), `${step} dB`]),
    String(settings.stepDb ?? 0.5),
  );
  element('operation-label').hidden = !recording;
  element('direction-label').hidden = !volume || dial;
  element('step-label').hidden = !volume;
  element('recording-settings').hidden = !recording;
  element('recording-folder').textContent = summary.folder;
  element('recording-format').textContent = summary.format;
  element('hint').textContent = hint ? t(hint) : '';
  element('hint').hidden = !hint;
}

function save() {
  socket.send(JSON.stringify({ event: 'setSettings', context, payload: model.settings }));
  render();
}
for (const field of ['graph', 'node', 'operation', 'direction', 'step']) {
  element(field).onchange = () => {
    model.select(field, element(field).value);
    save();
  };
}

window.connectElgatoStreamDeckSocket = (port, uuid, registerEvent, info, actionInfo) => {
  context = uuid;
  language = JSON.parse(info).application.language;
  document.documentElement.lang = language;
  const action = JSON.parse(actionInfo);
  actionId = action.action;
  model = new InspectorModel(actionId, action.payload.settings, action.payload.controller === 'Encoder', language);
  socket = new WebSocket(`ws://127.0.0.1:${port}`);
  socket.onopen = () => {
    socket.send(JSON.stringify({ event: registerEvent, uuid }));
    socket.send(
      JSON.stringify({
        event: 'sendToPlugin',
        action: actionId,
        context,
        payload: { type: 'snapshot' },
      }),
    );
  };
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.event === 'sendToPropertyInspector' && message.payload.type === 'snapshot') {
      model.snapshot = message.payload.snapshot;
      render();
    }
  };
  socket.onclose = () => {
    model.snapshot = undefined;
    render();
  };
  render();
};

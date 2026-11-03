import { readFileSync } from 'node:fs';
import streamDeck from '@elgato/streamdeck';
import { actionIds } from './action-settings.mjs';
import { exchange } from './client.mjs';
import { DeckControls } from './deck-controls.mjs';
import { keyImage } from './key-image.mjs';

const icons = new Map(
  Object.entries(actionIds).map(([name, id]) => [
    id,
    readFileSync(new URL(`../images/${name}.svg`, import.meta.url), 'utf8'),
  ]),
);

async function render(entry, view) {
  view.value = streamDeck.i18n.translate(view.value);
  if (entry.action.isDial()) {
    const value = { title: view.title, value: view.value };
    const image = JSON.stringify(value);
    if (image === entry.image) return;
    await entry.action.setFeedback(value);
    entry.image = image;
  } else {
    const image = keyImage(view, entry.settings.operation, icons.get(entry.action.manifestId));
    if (image === entry.image) return;
    await entry.action.setImage(image);
    entry.image = image;
  }
}

const controls = new DeckControls({
  control: {
    snapshot: () => exchange({ type: 'snapshot' }, undefined, 4000),
    send: exchange,
  },
  display: {
    render,
    snapshot: (snapshot) => streamDeck.ui.sendToPropertyInspector({ type: 'snapshot', snapshot }),
    alert: (action) => action.showAlert(),
    warn: (error) => streamDeck.logger.warn(String(error)),
  },
  now: () => performance.now(),
  wait: (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)),
  schedule: setTimeout,
});
const update = (event) => controls.update(event.action, event.payload.settings);
const control = (event, operation, ticks) => controls.control(event.action.id, operation, ticks);

streamDeck.actions.onWillAppear(update);
streamDeck.settings.onDidReceiveSettings(update);
streamDeck.actions.onWillDisappear((event) => controls.remove(event.action.id));
streamDeck.actions.onKeyDown((event) => {
  void control(event).catch((error) => streamDeck.logger.warn(String(error)));
});
streamDeck.actions.onDialDown((event) => {
  void control(event, 'mute').catch((error) => streamDeck.logger.warn(String(error)));
});
streamDeck.actions.onDialRotate((event) => {
  void control(event, 'gain_up', event.payload.ticks).catch((error) =>
    streamDeck.logger.warn(String(error)),
  );
});
streamDeck.ui.onSendToPlugin((event) => {
  if (event.payload?.type === 'snapshot')
    void controls.publish().catch((error) => streamDeck.logger.warn(String(error)));
});
await streamDeck.connect();
await controls.session.refresh();
setInterval(() => {
  void controls.session.refresh().catch((error) => streamDeck.logger.warn(String(error)));
}, 1000);

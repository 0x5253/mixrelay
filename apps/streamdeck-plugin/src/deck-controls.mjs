import { actionIds, actionSettings } from './action-settings.mjs';
import { ControlSession } from './control-session.mjs';
import { presentation } from './controls.mjs';
import { Feedback } from './feedback.mjs';
import { VolumeGesture } from './volume-gesture.mjs';

/** Owns visible actions and live control intent through exchange, display and time ports. */
export class DeckControls {
  #actions = new Map();

  constructor({ control, display, now, wait, schedule }) {
    this.display = display;
    this.session = new ControlSession(control, () => this.publish(), now);
    this.volume = new VolumeGesture(this.session, now, wait);
    this.feedback = new Feedback(
      (entry) => display.render(entry, presentation(this.session.snapshot, entry.settings))
        .catch((error) => display.warn(error)),
      schedule,
    );
  }

  update(action, settings) {
    this.volume.cancel(action.id);
    const entry = {
      ...this.#actions.get(action.id),
      action,
      settings: actionSettings(action.manifestId, settings, action.isDial()),
    };
    this.#actions.set(action.id, entry);
    this.feedback.update(entry);
  }

  remove(id) {
    this.volume.cancel(id);
    this.feedback.remove(id);
    this.#actions.delete(id);
  }

  async publish() {
    for (const entry of this.#actions.values()) this.feedback.update(entry);
    await this.display.snapshot(this.session.snapshot ?? null);
  }

  async control(id, operation, ticks) {
    const entry = this.#actions.get(id);
    if (!entry) return;
    try {
      if (operation !== undefined && entry.action.manifestId !== actionIds.volume)
        throw new Error('This action does not support dial controls');
      if (entry.action.manifestId === actionIds.volume && operation !== 'mute') {
        const delta = ticks ?? (entry.settings.operation === 'gain_down' ? -1 : 1);
        await this.volume.change(id, entry.settings, delta);
      } else {
        this.volume.cancel();
        await this.session.control(entry.settings, operation, ticks);
      }
    } catch (error) {
      this.display.warn(error);
      await this.display.alert(entry.action);
    }
  }
}

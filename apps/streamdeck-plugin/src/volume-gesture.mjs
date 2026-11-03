import { volumeStep } from './action-settings.mjs';
import { commandFor } from './control-command.mjs';
import { targetNode } from './control-target.mjs';
import { clampGainDb } from './gain.mjs';

const clamp = (gain) => Math.fround(clampGainDb(gain));

/** A short live gesture, not a command queue: only confirmed self-authored revisions can drain it. */
export class VolumeGesture {
  #gesture;

  constructor(session, now, wait) {
    this.session = session;
    this.now = now;
    this.wait = wait;
  }

  cancel(owner) {
    if (this.#gesture && (owner === undefined || this.#gesture.owner === owner))
      this.#gesture.cancelled = true;
  }

  change(owner, settings, ticks) {
    if (!Number.isSafeInteger(ticks) || Math.abs(ticks) > 256)
      return Promise.reject(new Error('Invalid dial ticks'));
    const key = JSON.stringify([owner, settings.graphId, settings.nodeId, volumeStep(settings)]);
    let gesture = this.#gesture;
    if (gesture && this.now() > gesture.expires) {
      this.cancel();
      return Promise.reject(new Error('Volume gesture expired'));
    }
    if (gesture && (gesture.key !== key || gesture.cancelled)) {
      this.cancel();
      return Promise.reject(new Error('Another volume gesture is still finishing'));
    }
    if (!gesture) {
      commandFor(this.session.snapshot, settings, 'gain_up', 0);
      const snapshot = this.session.snapshot;
      gesture = {
        key,
        owner,
        settings: { ...settings },
        session: snapshot.session,
        structure: snapshot.structureGeneration,
        revision: snapshot.revision,
        desired: targetNode(snapshot, settings).gainDb,
        cancelled: false,
      };
      this.#gesture = gesture;
    }
    // Clamp each event before merging the next; reversing at a limit must move immediately.
    gesture.desired = clamp(gesture.desired + ticks * volumeStep(settings));
    gesture.expires = this.now() + 750;
    gesture.task ??= this.#drain(gesture).finally(() => {
      if (this.#gesture === gesture) this.#gesture = undefined;
    });
    return gesture.task;
  }

  #check(gesture) {
    const snapshot = this.session.snapshot;
    const node = targetNode(snapshot, gesture.settings);
    if (
      gesture.cancelled ||
      this.now() > gesture.expires ||
      !snapshot ||
      snapshot.session !== gesture.session ||
      snapshot.structureGeneration !== gesture.structure ||
      !node?.control.enabled ||
      !Number.isFinite(node.gainDb) ||
      (snapshot.enginePower &&
        (!snapshot.enginePower.enabled || snapshot.enginePower.transitioning))
    )
      throw new Error('Volume gesture no longer matches the active target');
    return snapshot;
  }

  async #drain(gesture) {
    await Promise.resolve();
    for (;;) {
      const before = this.#check(gesture);
      if (before.revision !== gesture.revision || before.activeRevision < before.revision)
        throw new Error('Volume gesture revision changed');
      const desired = gesture.desired;
      if (targetNode(before, gesture.settings).gainDb === desired) return;
      const deadline = this.now() + 750;
      const response = await this.session.setGain(gesture.settings, desired);
      if (this.now() > deadline) throw new Error('Volume acknowledgement timed out');
      if (response.revision !== gesture.revision + 1 && response.revision !== gesture.revision)
        throw new Error('Unexpected volume acknowledgement');
      gesture.revision = response.revision;
      await this.#settle(gesture, desired, deadline);
      if (gesture.desired === desired) return;
    }
  }

  async #settle(gesture, desired, deadline) {
    for (;;) {
      if (this.now() > deadline) throw new Error('Volume acknowledgement timed out');
      const snapshot = this.#check(gesture);
      if (snapshot.revision !== gesture.revision)
        throw new Error('Volume gesture superseded by another command');
      if (snapshot.activeRevision >= gesture.revision) {
        if (targetNode(snapshot, gesture.settings).gainDb !== desired)
          throw new Error('Volume acknowledgement differs from requested gain');
        return;
      }
      await this.wait(25);
      await this.session.refresh();
    }
  }
}

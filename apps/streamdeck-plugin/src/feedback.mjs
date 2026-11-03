/** Coalesces display changes and serialises frames at no more than 10 per second. */
export class Feedback {
  #pending = new Map();
  #timer;
  #rendering = false;

  constructor(render, schedule) {
    this.render = render;
    this.schedule = schedule;
  }

  update(entry) {
    this.#pending.set(entry.action.id, entry);
    this.#schedule();
  }

  remove(id) {
    this.#pending.delete(id);
  }

  #schedule() {
    if (this.#timer !== undefined || this.#rendering || !this.#pending.size) return;
    this.#timer = this.schedule(() => this.#flush(), 100);
  }

  async #flush() {
    this.#timer = undefined;
    this.#rendering = true;
    const entries = [...this.#pending.values()];
    this.#pending.clear();
    await Promise.allSettled(entries.map(this.render));
    this.#rendering = false;
    this.#schedule();
  }
}

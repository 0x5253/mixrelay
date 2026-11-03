import assert from 'node:assert/strict';

export function permissionHandshake({ directory, alive, now, wait, nextId, channel, writeReceipt }) {
  async function transition(phase) {
    const command = { generation: nextId(), phase };
    channel.publish(command);
    const deadline = now() + 30000;
    for (;;) {
      assert.ok(alive(), `macOS permission observer exited during ${phase}`);
      const state = channel.read();
      if (state !== undefined) {
        if (state.generation === command.generation && state.phase === phase) {
          assert.ok(Number.isFinite(Date.parse(state.at)), 'macOS observer acknowledgement requires an execution timestamp');
          return state;
        }
      }
      assert.ok(now() < deadline, `macOS permission observer did not acknowledge ${phase}`);
      await wait(100);
    }
  }
  return async (measure, { receiptDirectory = directory } = {}) => {
    const errors = [];
    const receipt = {};
    let result;
    try {
      receipt.paused = await transition('paused');
      result = await measure();
    } catch (error) { errors.push(error); }
    try { receipt.resumed = await transition('observing'); }
    catch (error) { errors.push(error); }
    writeReceipt(receiptDirectory, receipt);
    if (errors.length === 1) throw errors[0];
    if (errors.length) throw new AggregateError(errors, errors.map(error => error.message).join('; '));
    return result;
  };
}

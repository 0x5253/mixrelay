import assert from 'node:assert/strict';

/** One desktop invocation; only complete pending replies extend its inactivity deadline. */
export function runtimeControl({ connect, schedule, cancel, path }) {
  return (name, arguments_) => new Promise((resolve, reject) => {
    const connection = connect(path);
    let parts = [], size = 0, finished = false, timer;
    const finish = (error, value) => {
      if (finished) return;
      finished = true;
      cancel(timer);
      connection.destroy();
      if (error) reject(error);
      else resolve(value);
    };
    const arm = () => {
      cancel(timer);
      timer = schedule(() => finish(new Error('Runtime command timed out; inspect graph before retrying')), 20000);
    };
    arm();
    connection.on('connect', () => connection.write(`${JSON.stringify({ id: 1, command: { type: 'invoke', name, arguments: arguments_ } })}\n`));
    connection.on('error', (error) => finish(error));
    connection.on('close', () => finish(new Error('Runtime disconnected; inspect graph before retrying')));
    connection.on('data', (chunk) => {
      if (finished) return;
      try {
        let offset = 0;
        while (!finished && offset < chunk.length) {
          const end = chunk.indexOf(10, offset);
          const segment = chunk.subarray(offset, end < 0 ? chunk.length : end);
          size += segment.length;
          assert.ok(size <= 4 * 1024 * 1024, 'Runtime response too large');
          if (segment.length) parts.push(segment);
          if (end < 0) return;
          const reply = JSON.parse(Buffer.concat(parts, size).toString('utf8'));
          parts = []; size = 0; offset = end + 1;
          assert.equal(reply.id, 1, 'Foreign runtime response');
          assert.ok(Number.isInteger(reply.status), 'Invalid runtime status');
          if (reply.status === 102) { arm(); continue; }
          assert.equal(reply.status, 200, JSON.stringify(reply));
          assert.ok(reply.body && !('error' in reply.body) && 'ok' in reply.body, JSON.stringify(reply));
          finish(null, reply.body.ok);
        }
      } catch (error) { finish(error); }
    });
  });
}

/** One HTTP exchange, with an absolute deadline and bounded response, never replayed. */
export function httpExchange({ request, schedule, cancel }) {
  return (socketPath, path, {
    method = 'GET', headers = {}, body, timeoutMs = 5000, responseLimit = 16 * 1024 * 1024,
  } = {}) => new Promise((resolve, reject) => {
    let outgoing, incoming, timer, settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      cancel(timer);
      if (error) {
        incoming?.destroy();
        outgoing?.destroy();
        reject(error);
      } else resolve(value);
    };
    try {
      outgoing = request(
        { socketPath, path, method, headers: { host: 'mixrelay', ...headers } },
        (response) => {
          incoming = response;
          let text = '', bytes = 0;
          response.setEncoding('utf8');
          response.on('data', (chunk) => {
            if (settled) return;
            bytes += new TextEncoder().encode(chunk).length;
            if (bytes > responseLimit) return finish(new Error(`${path}: response too large`));
            text += chunk;
          });
          response.on('end', () => finish(null,
            { status: response.statusCode, headers: response.headers, body: text }));
          response.on('error', (error) => finish(error));
          response.on('aborted', () => finish(new Error(`${path}: response aborted`)));
        },
      );
      outgoing.on('error', (error) => finish(error));
      timer = schedule(() => finish(new Error(`${path}: timed out`)), timeoutMs);
      outgoing.end(body);
    } catch (error) {
      finish(error);
    }
  });
}

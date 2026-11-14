// The same reader serves a GET started by the page and one started by a Rune task. Stop
// reading before allocating wasm memory. Only a bounded prefix goes to Rust on refusal.
export async function boundedBody(response, url) {
  const path = new URL(url, location.href).pathname;
  const message = /\/api\/session\/[^/]+\/message(?:\/|$)/.test(path);
  const limit = response.status >= 300 ? 65536 : message ? 32 << 20 : path.includes('/api/fs/read/') ? 16 << 20 : 64 << 20;
  const length = Number(response.headers.get('content-length'));
  const budget = response.status >= 300 ? limit : length > limit ? (message ? 8192 : 0) : limit + 1;
  const chunks = [];
  let size = 0;
  if (response.body) {
    const reader = response.body.getReader();
    try {
      while (size < budget) {
        const {done, value} = await reader.read();
        if (done) break;
        const n = Math.min(budget - size, value.length);
        chunks.push(n === value.length ? value : value.slice(0, n));
        size += n;
      }
      if (size === budget) await reader.cancel();
    } finally { reader.releaseLock(); }
  }
  const body = new Uint8Array(size);
  let pos = 0;
  for (const chunk of chunks) { body.set(chunk, pos); pos += chunk.length; }
  return {status: response.status, headers: [...response.headers], body};
}

export function readAhead(promise, url) {
  const entry = {done: undefined};
  entry.promise = promise.then(r => boundedBody(r, url)).then(done => (entry.done = done));
  entry.promise.catch(() => {});
  return entry;
}

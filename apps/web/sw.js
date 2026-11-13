// The page's own files, kept on the device: a second open takes the shell and the module from
// here instead of the network, and an open with no network still starts the app, which then says
// it can't reach the server. Only this build's files under the page's folder are answered; the
// server's API (on the same origin when the page is mounted beside it) always goes to the network.
// `scripts/web.ps1` writes the build's id and its file list in place of the two names below.
const BUILD = '__BUILD__';
// Fetched when the worker installs: what the first frame needs.
const SHELL = __SHELL__;
// Kept the first time the page asks for one: fonts, catalogs, the dictionary and sounds.
const LATER = /^(fonts|i18n|spell|sounds|icons|snippets)\//;
const CACHE = `rune-${BUILD}`;
const scope = new URL(self.registration.scope);

self.addEventListener('install', event => {
  event.waitUntil((async () => {
    const cache = await caches.open(CACHE);
    // `no-cache` revalidates with the server, so what the page just downloaded comes from the
    // browser's cache (a 304) instead of the network a second time.
    await Promise.all(SHELL.map(async path => {
      const url = new URL(path, scope);
      if (await cache.match(url)) return;
      const response = await fetch(url, { cache: 'no-cache' });
      if (response.ok) await cache.put(url, response);
    }));
    await self.skipWaiting();
  })());
});

self.addEventListener('activate', event => {
  event.waitUntil((async () => {
    for (const name of await caches.keys()) if (name.startsWith('rune-') && name !== CACHE) await caches.delete(name);
    await self.clients.claim();
  })());
});

self.addEventListener('fetch', event => {
  const request = event.request;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== scope.origin || !url.pathname.startsWith(scope.pathname)) return;
  const path = url.pathname.slice(scope.pathname.length);
  // The page itself, whatever its query (`?server=…&session=…`): the folder's index.
  const page = request.mode === 'navigate' && (path === '' || path === 'index.html');
  if (!page && !SHELL.includes(path) && !LATER.test(path)) return;
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const key = page ? new URL('./', scope) : new URL(path, scope);
    const kept = await cache.match(key);
    if (kept) return kept;
    const response = await fetch(page ? key : request);
    if (response.ok && (page || SHELL.includes(path) || LATER.test(path))) await cache.put(key, response.clone());
    return response;
  })());
});

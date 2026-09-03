// Offline shell for the installed PWA.
//
// Network-first for everything, cache as the fallback. The obvious alternative
// — cache-first for the shell — pins a viewer to whatever HTML they installed,
// and `agtx update` swaps the binary underneath them, so the app would keep
// serving a stale UI against a newer API with no way to notice. Paying one
// conditional request per asset is worth never being able to wedge that way.
//
// The API is deliberately not cached: a board is only useful when it is true,
// and a cached one would show yesterday's phase status with today's timestamp.

const CACHE = "agtx-shell-v1";
const SHELL = [
  "./",
  "index.html",
  "app.css",
  "app.js",
  "api.js",
  "manifest.webmanifest",
  "icon-192.png",
  "icon-512.png",
];

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== location.origin) return;
  if (url.pathname.startsWith("/api/")) return; // always live

  e.respondWith(
    fetch(e.request)
      .then((res) => {
        const copy = res.clone();
        caches.open(CACHE).then((c) => c.put(e.request, copy)).catch(() => {});
        return res;
      })
      .catch(() => caches.match(e.request).then((hit) => hit || caches.match("index.html"))),
  );
});

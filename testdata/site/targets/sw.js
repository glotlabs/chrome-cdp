// The service worker of the target matrix (scope /targets/). It has no
// fetch handler: pages' requests go to the network as they would.
importScripts('/targets/ops.js');
self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));
self.addEventListener('message', (e) => {
  e.waitUntil((async () => {
    e.source.postMessage({ id: e.data.id, result: await targetOps('service', e.data.op) });
  })());
});

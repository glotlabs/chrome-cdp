// A worker that opens a WebSocket to the matrix's mocked /targets/ws as
// the first thing it does, and posts the socket's first message (or
// `failed`): the mock shim must be there before its script (plan 13.2).
const ws = new WebSocket(new URL('/targets/ws', location.href).href.replace(/^http/, 'ws'));
const first = new Promise((resolve) => {
  ws.onmessage = (e) => resolve(e.data);
  ws.onerror = () => resolve('failed');
});
if (typeof SharedWorkerGlobalScope !== 'undefined' && self instanceof SharedWorkerGlobalScope) {
  onconnect = (e) => first.then((data) => e.ports[0].postMessage(data));
} else {
  first.then((data) => postMessage(data));
}

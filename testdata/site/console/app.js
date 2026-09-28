// What the buttons of console/index.html do.

function failSync() {
  const cart = null;
  return cart.items;
}

function failLater() {
  setTimeout(function later() {
    throw new Error('later failure');
  }, 0);
}

function rejectPromise() {
  Promise.reject(new Error('rejected promise'));
}

function rejectHandledLate() {
  const promise = Promise.reject(new Error('handled late'));
  setTimeout(() => promise.catch(() => {}), 50);
}

function fetchMissing() {
  fetch('missing.json');
}

function fetchCors() {
  // Origin B sends no CORS headers.
  fetch('{{ORIGIN_B}}/console/data.json').catch(() => {});
}

function fetchUnknownHost() {
  fetch('http://no-such-host.invalid/x').catch(() => {});
}

function fetchAborted() {
  const controller = new AbortController();
  fetch('data.json?delay=2000', { signal: controller.signal }).catch(() => {});
  controller.abort();
}

function fetchThirdParty() {
  fetch('{{ORIGIN_B}}/console/missing.json', { mode: 'no-cors' }).catch(() => {});
}

function burst(n) {
  for (let i = 0; i < n; i++) {
    console.log('burst', i);
  }
}

function startWorker() {
  new Worker('worker.js');
}

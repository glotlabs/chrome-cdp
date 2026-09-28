// One function per button of index.html; each sends one kind of request.

const out = document.getElementById('out');

function loadItems() {
  fetch('/items?n=300')
    .then((r) => r.json())
    .then((items) => { out.textContent = `${items.length} items`; });
}

function logIn() {
  fetch('/api/login?status=500', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Test': 'yes' },
    body: JSON.stringify({ user: 'ada', password: 's3cret' }),
  }).catch(() => {});
}

function upload() {
  const xhr = new XMLHttpRequest();
  xhr.open('POST', '/items?n=1');
  xhr.setRequestHeader('Content-Type', 'text/plain');
  xhr.send('x'.repeat(20000));
}

function fetchMissing() {
  fetch('missing.json').catch(() => {});
}

function fetchRefused() {
  fetch('http://127.0.0.1:2/refused').catch(() => {});
}

function fetchCors() {
  fetch('{{ORIGIN_B}}/items?n=1').catch(() => {});
}

function fetchAborted() {
  const controller = new AbortController();
  fetch('/items?n=1&delay=2000', { signal: controller.signal }).catch(() => {});
  setTimeout(() => controller.abort(), 100);
}

function fetchSlow() {
  fetch('/items?n=1&delay=1500').then((r) => r.text()).then(() => { out.textContent = 'slow done'; });
}

function fetchRedirect() {
  fetch('/redirect?to=/items?n=1').catch(() => {});
}

function echo() {
  const ws = new WebSocket(`${location.origin.replace('http', 'ws')}/ws`);
  ws.onopen = () => ws.send('hello');
  ws.onmessage = (e) => {
    out.textContent = `echo: ${e.data}`;
    ws.close();
  };
}

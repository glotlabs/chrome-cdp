// Agent benchmark pages (plan 10.4) send what the user submitted here; the
// benchmark's verifier reads it back from the fixture server.
window.record = function (data) {
  return fetch('/bench/record', { method: 'POST', body: JSON.stringify(data) });
};

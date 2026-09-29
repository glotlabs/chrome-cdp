onmessage = async (e) => {
  if (e.data === 'fetch') {
    const response = await fetch('/workers/missing.json');
    postMessage('fetched ' + response.status);
    return;
  }
  postMessage(e.data === 'ping' ? 'pong' : 'unexpected ' + e.data);
};

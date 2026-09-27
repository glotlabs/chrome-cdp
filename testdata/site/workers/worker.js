onmessage = (e) => postMessage(e.data === 'ping' ? 'pong' : 'unexpected ' + e.data);

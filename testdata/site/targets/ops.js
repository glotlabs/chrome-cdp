// What every frame and worker of the target matrix (plan 13) does when
// asked: `targetOps(who, op)` answers with a value to post back. A
// request that fails answers `failed: <message>`.
self.targetOps = async (who, op) => {
  try {
    return await targetRun(who, op);
  } catch (e) {
    return 'failed: ' + e.message;
  }
};

// A WebSocket to the matrix's mocked `/targets/ws`, once it has opened
// and said hello.
const targetSocket = (binaryType = 'arraybuffer') =>
  new Promise((resolve, reject) => {
    const ws = new WebSocket(new URL('/targets/ws', location.href).href.replace(/^http/, 'ws'));
    ws.binaryType = binaryType;
    ws.onmessage = (e) => (e.data === 'hello' ? resolve(ws) : reject(new Error('said ' + e.data)));
    ws.onerror = () => reject(new Error('the WebSocket failed'));
  });

// The next message `ws` gets.
const targetNext = (ws) => new Promise((resolve) => ws.addEventListener('message', (e) => resolve(e.data), { once: true }));

async function targetRun(who, op) {
  switch (op) {
    case 'ping':
      return 'pong';
    case 'log':
      console.log('log from ' + who);
      return 'logged';
    case 'throw':
      setTimeout(() => { throw new Error('thrown in ' + who); });
      return 'thrown';
    case 'fetch': {
      const response = await fetch('/targets/api?from=' + encodeURIComponent(who));
      return (await response.json()).from;
    }
    case 'unmatched': {
      // No mock has it, but the matrix's mock set owns its path.
      const response = await fetch('/targets/unmatched?from=' + encodeURIComponent(who));
      return response.status;
    }
    case 'auth': {
      const response = await fetch('/auth/index.html?from=' + encodeURIComponent(who));
      return response.status;
    }
    case 'ws': {
      const ws = await targetSocket();
      const answer = targetNext(ws);
      ws.send('ping');
      const text = await answer;
      ws.close();
      return text;
    }
    case 'ws-binary': {
      const ws = await targetSocket();
      const answer = targetNext(ws);
      ws.send(new Uint8Array([1, 2, 3]));
      const data = await answer;
      ws.close();
      return data instanceof ArrayBuffer ? Array.from(new Uint8Array(data)).join(',') : 'text ' + data;
    }
    case 'ws-blob': {
      // The same, as Blobs both ways.
      const ws = await targetSocket('blob');
      const answer = targetNext(ws);
      ws.send(new Blob([new Uint8Array([1, 2, 3])]));
      const data = await answer;
      ws.close();
      if (!(data instanceof Blob)) return 'not a Blob: ' + data;
      return Array.from(new Uint8Array(await data.arrayBuffer())).join(',');
    }
    case 'sse': {
      if (typeof EventSource !== 'function') return 'no EventSource';
      const events = new EventSource('/targets/sse');
      const data = await new Promise((resolve) => {
        events.onmessage = (e) => resolve(e.data);
        events.onerror = () => resolve('failed');
      });
      events.close();
      return data;
    }
    case 'init':
      // What the matrix's init script set, if it ran here.
      return self.matrixInit ?? null;
    case 'headers': {
      // What its requests carry.
      const response = await fetch('/echo/targets');
      const { headers } = await response.json();
      return { userAgent: headers['user-agent'], language: headers['accept-language'] };
    }
    case 'report':
      return {
        userAgent: navigator.userAgent,
        language: navigator.language,
        timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
        now: Date.now(),
        random: Math.random(),
      };
    default:
      return 'unknown op ' + op;
  }
}

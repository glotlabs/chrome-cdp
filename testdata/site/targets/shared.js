// A shared worker of the target matrix; `?who=` names it.
importScripts('/targets/ops.js');
const who = new URLSearchParams(location.search).get('who') || 'shared';
onconnect = (e) => {
  const port = e.ports[0];
  port.onmessage = async (m) => port.postMessage({ id: m.data.id, result: await targetOps(who, m.data.op) });
  port.postMessage({ ready: who });
};

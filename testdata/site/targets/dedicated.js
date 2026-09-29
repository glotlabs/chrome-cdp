// A dedicated worker of the target matrix; `?who=` names it.
importScripts('/targets/ops.js');
const who = new URLSearchParams(location.search).get('who') || 'dedicated';
onmessage = async (e) => postMessage({ id: e.data.id, result: await targetOps(who, e.data.op) });
postMessage({ ready: who });

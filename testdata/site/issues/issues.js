// Issues the page raises as it loads (see index.html).

// A deprecated API: a synchronous request on the main thread.
const sync = new XMLHttpRequest();
sync.open('GET', '/issues/data.json', false);
sync.send();

// A request to the other site, which sends no CORS headers: blocked.
fetch('{{ORIGIN_B}}/issues/data.json').catch(() => {});

// And one from a worker.
new Worker('worker.js');

// A worker's request to the other site, which sends no CORS headers.
fetch('{{ORIGIN_B}}/issues/worker-data.json').catch(() => {});

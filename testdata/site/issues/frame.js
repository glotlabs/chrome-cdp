// The frame's own requests to the page's site, which sends no CORS headers.
fetch('{{ORIGIN_A}}/issues/data.json?from=frame').catch(() => {});

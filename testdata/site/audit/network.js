// A request of the page's own origin that gets a 404 (its body read, so
// the request ends).
fetch('/audit/missing.json').then((r) => r.text()).catch(() => {});

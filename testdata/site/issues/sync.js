// A deprecated API: a synchronous request on the main thread.
const request = new XMLHttpRequest();
request.open('GET', '/issues/data.json', false);
request.send();

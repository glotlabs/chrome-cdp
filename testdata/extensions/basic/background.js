// Answers the popup, so a test can see the service worker run.
chrome.runtime.onMessage.addListener((message, _sender, reply) => {
  if (message === "ping") reply("pong from the service worker");
});

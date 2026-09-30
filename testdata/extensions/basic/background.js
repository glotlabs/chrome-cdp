// Answers the popup, so a test can see the service worker run, and logs
// it, so a test can see its console. "boom" makes it fail.
chrome.runtime.onMessage.addListener((message, _sender, reply) => {
  if (message === "ping") {
    console.log("chrome-cdp fixture: ping from the popup");
    reply("pong from the service worker");
  } else if (message === "boom") {
    console.error("chrome-cdp fixture: boom");
    reply("boomed");
  }
});

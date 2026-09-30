document.getElementById("name").textContent = chrome.runtime.getManifest().name;
chrome.runtime.sendMessage("ping").then((reply) => {
  document.getElementById("reply").textContent = reply;
});
// Counts its openings in chrome.storage.local, so a test can see a write.
chrome.storage.local.get("opened").then(async ({ opened = 0 }) => {
  await chrome.storage.local.set({ opened: opened + 1 });
  document.getElementById("opened").textContent = `opened ${opened + 1} times`;
});

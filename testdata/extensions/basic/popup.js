document.getElementById("name").textContent = chrome.runtime.getManifest().name;
chrome.runtime.sendMessage("ping").then((reply) => {
  document.getElementById("reply").textContent = reply;
});

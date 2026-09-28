// `used` runs when the button is clicked; `unused` never does.
function used() {
  document.getElementById('out').textContent = 'ran';
}

function unused() {
  const parts = [];
  for (let i = 0; i < 100; i += 1) {
    parts.push(`this code never runs ${i}`);
  }
  return parts.join('\n');
}

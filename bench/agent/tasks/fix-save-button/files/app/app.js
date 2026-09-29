// The notes app: keeps notes in memory and lists them.
const notes = [];

function render() {
  document.getElementById('notes').innerHTML = notes
    .map((n) => `<li><strong>${n.title}</strong>: ${n.body}</li>`)
    .join('');
}

function save() {
  const title = document.getElementById('title').value.trim();
  const body = document.getElementById('body').value.trim();
  if (!title) {
    document.getElementById('status').textContent = 'A note needs a title.';
    return;
  }
  notes.push({ title, body });
  render();
  document.getElementById('status').textContent = `Saved "${title}"`;
}

document.querySelector('#save-button').addEventListener('click', save);
render();

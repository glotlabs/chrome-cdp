// Every fixture page of actions/ logs the clicks its buttons get (their
// ids, space-separated) into #log, so a test can check where a click at
// the pipeline's point landed.
document.addEventListener('click', (e) => {
  const el = e.target.closest('[id]');
  const log = document.getElementById('log');
  if (el && log && el !== log) log.textContent = (log.textContent + ' ' + el.id).trim();
});

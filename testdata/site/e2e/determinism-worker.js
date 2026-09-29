// What a worker of determinism.html sees (plan 13.3): its first random
// value, the time, the time zone and the language.
postMessage([
  Math.random(),
  new Date().toISOString(),
  Intl.DateTimeFormat().resolvedOptions().timeZone,
  navigator.language,
].join(' '));

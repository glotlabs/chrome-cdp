// A module the app loads later, in a chunk of its own (for logpoints
// set before their script loads).

export function later(why: string): string {
  const note = `later: ${why}`;
  document.title = note;
  return note;
}

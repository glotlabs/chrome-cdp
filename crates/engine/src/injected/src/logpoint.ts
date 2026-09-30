// Logpoints (plan 15, milestone 15.4). Built by `cargo xtask build-js`
// into its own script, `logpoint.js`, which is not installed in any
// world: the engine puts it in the condition of each logpoint's
// breakpoint, which V8 runs in the scope of the paused code whenever the
// line runs. The condition calls `hit` with one function per expression
// (closures over that scope), then returns false, so the page never
// stops. `hit` previews each value with the limits `components` uses and
// sends them to the engine on a CDP binding whose name the page cannot
// guess. It leaves nothing on the page's globals.

import { preview } from './frameworks';

/** An expression's value, previewed, or the error it threw. */
interface Shown {
  value?: string;
  error?: string;
}

/** What threw, as `<ReferenceError: total is not defined>` shows it. */
function thrown(error: unknown): string {
  try {
    if (error instanceof Error) return `${error.name}: ${error.message}`;
    return `Uncaught ${preview(error)}`;
  } catch {
    return 'error';
  }
}

/** Logpoint `id` was hit: each of `values` is run in its own `try` and
 * previewed, and the lot is sent on `binding` as
 * `{"id": 1, "values": [{"value": "12"}, {"error": "…"}]}`. */
export function hit(binding: string, id: number, values: Array<() => unknown>): void {
  const shown: Shown[] = values.map((value) => {
    try {
      return { value: preview(value()) };
    } catch (e) {
      return { error: thrown(e) };
    }
  });
  const send = (globalThis as Record<string, unknown>)[binding];
  if (typeof send === 'function') send(JSON.stringify({ id, values: shown }));
}

// Helpers the eval tests call from their scripts.
interface Totals {
  sum: number;
}

export function explode(): number {
  const totals: Totals | null = null;
  return totals!.sum;
}

(window as unknown as { explode: typeof explode }).explode = explode;

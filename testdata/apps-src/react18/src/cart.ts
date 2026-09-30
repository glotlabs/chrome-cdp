// The cart's arithmetic, the same in every fixture app.

import type { Item } from "./api";

export interface Line {
  id: number;
  qty: number;
}

let clicks = 0;

// Called on every click (for logpoints).
export function countClick(what: string): number {
  clicks += 1;
  return clicks;
}

// The price of `qty` of `item`. A missing item throws a TypeError (for
// locals at uncaught errors).
export function lineTotal(item: Item, qty: number): number {
  return item.price * qty;
}

export function total(items: Item[], lines: Line[]): number {
  let sum = 0;
  for (const line of lines) {
    const item = items.find((i) => i.id === line.id)!;
    sum += lineTotal(item, line.qty);
  }
  return sum;
}

// What "Checkout" totals: the cart and a gift card the catalog does not
// have, so `total` throws.
export function checkoutLines(items: Item[]): Line[] {
  return [...items.map((i) => ({ id: i.id, qty: i.qty })), { id: 99, qty: 1 }];
}

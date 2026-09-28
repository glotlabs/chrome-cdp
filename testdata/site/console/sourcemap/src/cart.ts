export interface Cart {
  items: string[];
  coupon?: { code: string };
}

export function checkout(cart: Cart): string {
  const code = cart.coupon!.code;
  return `${cart.items.length} items, coupon ${code}`;
}

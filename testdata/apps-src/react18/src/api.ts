// The cart's data, from cart.json next to the page.

export interface Item {
  id: number;
  name: string;
  price: number;
  qty: number;
}

// Loads the cart: on load, and after each "Add" (for request
// initiators).
export async function fetchCart(): Promise<Item[]> {
  const response = await fetch("cart.json");
  const body = await response.json();
  return body.items;
}

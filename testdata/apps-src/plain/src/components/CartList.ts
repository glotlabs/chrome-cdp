import type { Item } from "../api";
import { CartItem } from "./CartItem";

export interface Actions {
  add: (id: number) => void;
  remove: (id: number) => void;
}

export function CartList(items: Item[], actions: Actions): HTMLElement {
  const list = document.createElement("ul");
  list.dataset.testid = "cart-list";
  list.append(...items.map((item) => CartItem(item, actions)));
  return list;
}

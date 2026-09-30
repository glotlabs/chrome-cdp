import type { Item } from "../api";
import { countClick } from "../cart";
import type { Actions } from "./CartList";

export function CartItem(item: Item, actions: Actions): HTMLElement {
  const row = document.createElement("li");
  row.dataset.testid = `item-${item.id}`;
  const name = document.createElement("span");
  name.dataset.testid = `name-${item.id}`;
  name.textContent = item.name;
  const qty = document.createElement("span");
  qty.dataset.testid = `qty-${item.id}`;
  qty.textContent = String(item.qty);
  const add = document.createElement("button");
  add.dataset.testid = `add-${item.id}`;
  add.textContent = "Add";
  add.addEventListener("click", () => {
    countClick("add");
    actions.add(item.id);
  });
  const remove = document.createElement("button");
  remove.dataset.testid = `remove-${item.id}`;
  remove.textContent = "Remove";
  remove.addEventListener("click", () => {
    countClick("remove");
    actions.remove(item.id);
  });
  row.append(name, " × ", qty, " ", add, " ", remove);
  return row;
}

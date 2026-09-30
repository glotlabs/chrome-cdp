import type { Item } from "./api";
import { checkoutLines, countClick, renderTotal, total } from "./cart";
import { CartList, type Actions } from "./components/CartList";

export function App(items: Item[], actions: Actions): HTMLElement {
  const app = document.createElement("main");
  app.dataset.testid = "cart";
  const title = document.createElement("h1");
  title.dataset.testid = "title";
  title.textContent = "Cart";
  const status = document.createElement("p");
  status.dataset.testid = "status";
  const checkout = document.createElement("button");
  checkout.dataset.testid = "checkout";
  checkout.textContent = "Checkout";
  checkout.addEventListener("click", () => {
    countClick("checkout");
    status.textContent = `Paid ${total(items, checkoutLines(items))}`;
  });
  app.append(title, CartList(items, actions), renderTotal(items), checkout, status);
  return app;
}

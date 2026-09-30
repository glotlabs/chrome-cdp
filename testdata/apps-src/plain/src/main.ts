import { fetchCart, type Item } from "./api";
import { App } from "./App";

const root = document.getElementById("app")!;
let items: Item[] = [];

// Draws the app again from `items`.
function render() {
  root.replaceChildren(
    App(items, {
      add: async (id) => {
        items = items.map((i) => (i.id === id ? { ...i, qty: i.qty + 1 } : i));
        render();
        const fresh = await fetchCart();
        items = items.map((i) => ({ ...i, price: fresh.find((f) => f.id === i.id)?.price ?? i.price }));
        render();
      },
      remove: (id) => {
        items = items.filter((i) => i.id !== id);
        render();
      },
    }),
  );
}

render();
fetchCart().then((loaded) => {
  items = loaded;
  render();
});

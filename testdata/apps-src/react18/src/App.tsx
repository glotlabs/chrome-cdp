import { useEffect, useState } from "react";
import { fetchCart, type Item } from "./api";
import { checkoutLines, countClick, total } from "./cart";
import { CartList } from "./components/CartList";

export function App() {
  const [items, setItems] = useState<Item[]>([]);
  const [status, setStatus] = useState("");

  useEffect(() => {
    fetchCart().then(setItems);
  }, []);

  async function add(id: number) {
    setItems((all) => all.map((i) => (i.id === id ? { ...i, qty: i.qty + 1 } : i)));
    const fresh = await fetchCart();
    setItems((all) =>
      all.map((i) => ({ ...i, price: fresh.find((f) => f.id === i.id)?.price ?? i.price })),
    );
  }

  function remove(id: number) {
    setItems((all) => all.filter((i) => i.id !== id));
  }

  function checkout() {
    countClick("checkout");
    setStatus(`Paid ${total(items, checkoutLines(items))}`);
  }

  return (
    <main data-testid="cart">
      <h1 data-testid="title">Cart</h1>
      <CartList items={items} onAdd={add} onRemove={remove} />
      <p data-testid="total">Total: {total(items, items)}</p>
      <button data-testid="checkout" onClick={checkout}>
        Checkout
      </button>
      <p data-testid="status">{status}</p>
    </main>
  );
}

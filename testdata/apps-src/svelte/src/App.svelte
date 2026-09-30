<script lang="ts">
  import { onMount } from "svelte";
  import { fetchCart, type Item } from "./api";
  import { checkoutLines, countClick, total } from "./cart";
  import CartList from "./components/CartList.svelte";

  let items = $state<Item[]>([]);
  let status = $state("");

  onMount(async () => {
    items = await fetchCart();
  });

  async function add(id: number) {
    items = items.map((i) => (i.id === id ? { ...i, qty: i.qty + 1 } : i));
    const fresh = await fetchCart();
    items = items.map((i) => ({
      ...i,
      price: fresh.find((f) => f.id === i.id)?.price ?? i.price,
    }));
  }

  function remove(id: number) {
    items = items.filter((i) => i.id !== id);
  }

  function checkout() {
    countClick("checkout");
    status = `Paid ${total(items, checkoutLines(items))}`;
  }
</script>

<main data-testid="cart">
  <h1 data-testid="title">Cart</h1>
  <CartList {items} onadd={add} onremove={remove} />
  <p data-testid="total">Total: {total(items, items)}</p>
  <button data-testid="checkout" onclick={checkout}>Checkout</button>
  <p data-testid="status">{status}</p>
</main>

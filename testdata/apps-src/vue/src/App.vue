<script setup lang="ts">
import { onMounted, ref } from "vue";
import { fetchCart, type Item } from "./api";
import { checkoutLines, countClick, total } from "./cart";
import CartList from "./components/CartList.vue";

const items = ref<Item[]>([]);
const status = ref("");

onMounted(async () => {
  items.value = await fetchCart();
});

async function add(id: number) {
  items.value = items.value.map((i) => (i.id === id ? { ...i, qty: i.qty + 1 } : i));
  const fresh = await fetchCart();
  items.value = items.value.map((i) => ({
    ...i,
    price: fresh.find((f) => f.id === i.id)?.price ?? i.price,
  }));
}

function remove(id: number) {
  items.value = items.value.filter((i) => i.id !== id);
}

function checkout() {
  countClick("checkout");
  status.value = `Paid ${total(items.value, checkoutLines(items.value))}`;
}
</script>

<template>
  <main data-testid="cart">
    <h1 data-testid="title">Cart</h1>
    <CartList :items="items" @add="add" @remove="remove" />
    <p data-testid="total">Total: {{ total(items, items) }}</p>
    <button data-testid="checkout" @click="checkout">Checkout</button>
    <p data-testid="status">{{ status }}</p>
  </main>
</template>

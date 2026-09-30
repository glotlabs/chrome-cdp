<script setup lang="ts">
import type { Item } from "../api";
import CartItem from "./CartItem.vue";

defineProps<{ items: Item[] }>();
const emit = defineEmits<{ add: [id: number]; remove: [id: number] }>();
</script>

<template>
  <ul data-testid="cart-list">
    <CartItem
      v-for="item in items"
      :key="item.id"
      :item="item"
      @add="emit('add', $event)"
      @remove="emit('remove', $event)"
    />
  </ul>
</template>

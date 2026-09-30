<script setup lang="ts">
import type { Item } from "../api";
import { countClick } from "../cart";

const props = defineProps<{ item: Item }>();
const emit = defineEmits<{ add: [id: number]; remove: [id: number] }>();

function add() {
  countClick("add");
  emit("add", props.item.id);
}

function remove() {
  countClick("remove");
  emit("remove", props.item.id);
}
</script>

<template>
  <li :data-testid="`item-${item.id}`">
    <span :data-testid="`name-${item.id}`">{{ item.name }}</span> ×
    <span :data-testid="`qty-${item.id}`">{{ item.qty }}</span>
    <button :data-testid="`add-${item.id}`" @click="add">Add</button>
    <button :data-testid="`remove-${item.id}`" @click="remove">Remove</button>
  </li>
</template>

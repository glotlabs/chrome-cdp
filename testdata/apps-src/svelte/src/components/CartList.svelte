<script lang="ts">
  import type { Item } from "../api";
  import CartItem from "./CartItem.svelte";

  let {
    items,
    onadd,
    onremove,
  }: { items: Item[]; onadd: (id: number) => void; onremove: (id: number) => void } = $props();
</script>

<ul data-testid="cart-list">
  {#each items as item (item.id)}
    <CartItem {item} {onadd} {onremove} />
  {/each}
</ul>

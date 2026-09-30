<script lang="ts">
  import type { Item } from "../api";
  import { countClick } from "../cart";

  let {
    item,
    onadd,
    onremove,
  }: { item: Item; onadd: (id: number) => void; onremove: (id: number) => void } = $props();

  function add() {
    countClick("add");
    onadd(item.id);
  }

  function remove() {
    countClick("remove");
    onremove(item.id);
  }
</script>

<li data-testid={`item-${item.id}`}>
  <span data-testid={`name-${item.id}`}>{item.name}</span> ×
  <span data-testid={`qty-${item.id}`}>{item.qty}</span>
  <button data-testid={`add-${item.id}`} onclick={add}>Add</button>
  <button data-testid={`remove-${item.id}`} onclick={remove}>Remove</button>
</li>

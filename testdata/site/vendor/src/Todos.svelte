<script>
  // The same app as ../../apps/react.html and ../../apps/vue.html.
  let todos = $state(null);
  let draft = $state('');
  let filter = $state('all');
  let next = 100;

  fetch('/items?n=3&delay=300')
    .then((r) => r.json())
    .then((items) => (todos = items.map(({ id, name, done }) => ({ id, name, done }))));

  const shown = $derived(
    (todos ?? []).filter((t) => filter === 'all' || (filter === 'done') === t.done)
  );
  const left = $derived((todos ?? []).filter((t) => !t.done).length);

  function add(event) {
    event.preventDefault();
    const name = draft.trim();
    if (!name) return;
    todos.push({ id: next++, name, done: false });
    draft = '';
  }
</script>

<h1>Svelte todos</h1>
<form onsubmit={add}>
  <label for="new-todo">New todo</label>
  <input id="new-todo" bind:value={draft} autocomplete="off" />
  <button type="submit">Add</button>
</form>
{#if todos === null}
  <p role="status">Loading…</p>
{:else}
  <ul>
    {#each shown as todo (todo.id)}
      <li class:done={todo.done}>
        <label><input type="checkbox" bind:checked={todo.done} /> {todo.name}</label>
        <button aria-label={`Delete ${todo.name}`} onclick={() => (todos = todos.filter((t) => t !== todo))}>×</button>
      </li>
    {/each}
  </ul>
  <p id="left">{left} {left === 1 ? 'item' : 'items'} left</p>
  <nav aria-label="Filter">
    {#each ['all', 'active', 'done'] as f}
      <button aria-pressed={filter === f} onclick={() => (filter = f)}>{f[0].toUpperCase() + f.slice(1)}</button>
    {/each}
  </nav>
{/if}

<script>
  let name = $state('');
</script>

<label for="svelte-name">Svelte name</label>
<input id="svelte-name" bind:value={name} />
<p id="svelte-echo">{name}</p>

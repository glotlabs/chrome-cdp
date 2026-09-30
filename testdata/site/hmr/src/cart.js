// The module the fake hot updates re-import: each run counts one more.
window.cartRuns = (window.cartRuns || 0) + 1;
document.getElementById('cart').textContent = `cart module ran ${window.cartRuns} times`;
export const runs = window.cartRuns;

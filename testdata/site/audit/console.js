// Logs an error now, and again from the same line each time the page
// calls retryCart().
function retryCart() { console.error('Cart failed to load'); }
retryCart();

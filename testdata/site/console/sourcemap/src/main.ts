import { checkout, type Cart } from './cart';

const cart: Cart = { items: ['book'] };

document.querySelector('#checkout')?.addEventListener('click', () => {
  console.log('checking out', cart.items.length);
  checkout(cart);
});

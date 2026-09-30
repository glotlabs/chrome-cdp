import type { Item } from "../api";
import { CartItem } from "./CartItem";

interface Props {
  items: Item[];
  onAdd: (id: number) => void;
  onRemove: (id: number) => void;
}

export function CartList({ items, onAdd, onRemove }: Props) {
  return (
    <ul data-testid="cart-list">
      {items.map((item) => (
        <CartItem key={item.id} item={item} onAdd={onAdd} onRemove={onRemove} />
      ))}
    </ul>
  );
}

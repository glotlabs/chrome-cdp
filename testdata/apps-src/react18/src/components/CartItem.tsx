import type { Item } from "../api";
import { countClick } from "../cart";

interface Props {
  item: Item;
  onAdd: (id: number) => void;
  onRemove: (id: number) => void;
}

export function CartItem({ item, onAdd, onRemove }: Props) {
  return (
    <li data-testid={`item-${item.id}`}>
      <span data-testid={`name-${item.id}`}>{item.name}</span> ×{" "}
      <span data-testid={`qty-${item.id}`}>{item.qty}</span>{" "}
      <button
        data-testid={`add-${item.id}`}
        onClick={() => {
          countClick("add");
          onAdd(item.id);
        }}
      >
        Add
      </button>{" "}
      <button
        data-testid={`remove-${item.id}`}
        onClick={() => {
          countClick("remove");
          onRemove(item.id);
        }}
      >
        Remove
      </button>
    </li>
  );
}

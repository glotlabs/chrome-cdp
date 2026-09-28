// The shop's REST API. In the tests, chrome-cdp answers these requests
// from e2e/mocks (mock mode) or lets them through to the real API
// (`--backend local`).

export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

async function request(method, path, body) {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? {} : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    throw new ApiError(response.status, data?.error ?? `HTTP ${response.status}`);
  }
  return data;
}

export const api = {
  products: () => request("GET", "/api/products"),
  product: (id) => request("GET", `/api/products/${encodeURIComponent(id)}`),
  search: (q) => request("GET", `/api/search?q=${encodeURIComponent(q)}`),
  me: () => request("GET", "/api/me"),
  login: (email, password) => request("POST", "/api/login", { email, password }),
  logout: () => request("POST", "/api/logout"),
  cart: () => request("GET", "/api/cart"),
  addToCart: (product) =>
    request("POST", "/api/cart", {
      product_id: product.id,
      name: product.name,
      price: product.price,
      qty: 1,
    }),
  removeFromCart: (id) => request("DELETE", `/api/cart/${id}`),
  placeOrder: (items, total) => request("POST", "/api/orders", { items, total }),
  order: (id) => request("GET", `/api/orders/${encodeURIComponent(id)}`),
};

/// Live prices: `{"type": "price", "id": 1, "price": 79}` messages.
export function livePrices(onPrice) {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${scheme}://${location.host}/api/live`);
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.type === "price") onPrice(message);
  });
  return () => socket.close();
}

export const money = (n) => `$${Number(n).toFixed(2)}`;

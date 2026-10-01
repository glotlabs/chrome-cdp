// The shop's REST API, and the payment provider checkout charges the card
// at. In the tests, chrome-cdp answers these requests from e2e/mocks (mock
// mode), lets them through to the real API and provider (`--backend
// local`), or mocks only the provider (`--backend hybrid`).

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

// The payment provider, a third party on another origin (its URL is in
// public/config.js), called from the browser as Stripe.js would be.
export const payments = {
  charge: async (amount, card) => {
    const url = `${window.SHOP_CONFIG?.paymentsUrl ?? ""}/v1/charges`;
    const response = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        amount: Math.round(amount * 100),
        currency: "usd",
        card: card.replace(/\s/g, ""),
      }),
    });
    const data = await response.json().catch(() => null);
    if (!response.ok) {
      throw new ApiError(response.status, data?.error?.message ?? `HTTP ${response.status}`);
    }
    return data;
  },
};

export const money = (n) => `$${Number(n).toFixed(2)}`;

import { useCallback, useEffect, useState } from "react";
import { ApiError, api, livePrices, money, payments } from "./api.js";
import { Link, match, navigate, useLocation } from "./router.jsx";

export function App() {
  const location = useLocation();
  const [user, setUser] = useState(null);
  const [cart, setCart] = useState([]);

  const refreshCart = useCallback(() => api.cart().then(setCart), []);
  useEffect(() => {
    api.me().then((me) => setUser(me.user), () => setUser(null));
    refreshCart().catch(() => setCart([]));
  }, [refreshCart]);

  const shop = { user, setUser, cart, refreshCart };
  return (
    <>
      <Header {...shop} />
      <main>{page(location, shop)}</main>
    </>
  );
}

function page(location, shop) {
  const path = location.pathname;
  let params;
  if (path === "/") return <Products />;
  if ((params = match("/products/:id", path))) return <Product id={params.id} {...shop} />;
  if (path === "/search") return <Search q={location.searchParams.get("q") ?? ""} />;
  if (path === "/login") return <Login next={location.searchParams.get("next")} {...shop} />;
  if (path === "/cart") return <Cart {...shop} />;
  if (path === "/checkout") return <Checkout {...shop} />;
  if ((params = match("/orders/:id", path))) return <Order id={params.id} />;
  return <h1>Page not found</h1>;
}

function Header({ user, setUser, cart }) {
  const [q, setQ] = useState("");
  const search = (event) => {
    event.preventDefault();
    navigate(`/search?q=${encodeURIComponent(q)}`);
  };
  const signOut = async () => {
    await api.logout();
    setUser(null);
  };
  return (
    <header>
      <nav>
        <Link to="/">Shop</Link>
        <Link to="/cart">Cart ({cart.length})</Link>
      </nav>
      <form role="search" onSubmit={search}>
        <input type="search" aria-label="Search" value={q} onChange={(e) => setQ(e.target.value)} />
        <button className="button">Search</button>
      </form>
      {user ? (
        <p>
          Signed in as {user.email}{" "}
          <button className="button" onClick={signOut}>
            Sign out
          </button>
        </p>
      ) : (
        <Link to="/login">Sign in</Link>
      )}
    </header>
  );
}

/// Loads `load()` whenever `deps` change: `{ data, error, retry }`.
function useLoad(load, deps) {
  const [state, setState] = useState({ data: null, error: null });
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let live = true;
    setState({ data: null, error: null });
    load().then(
      (data) => live && setState({ data, error: null }),
      (error) => live && setState({ data: null, error }),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, attempt]);
  return { ...state, retry: () => setAttempt((n) => n + 1) };
}

function ErrorPage({ error, retry }) {
  return (
    <>
      <h1>Something went wrong</h1>
      <p role="alert">{error.message}</p>
      <button className="button" onClick={retry}>
        Try again
      </button>
    </>
  );
}

function Products() {
  const { data, error, retry } = useLoad(api.products, []);
  if (error) return <ErrorPage error={error} retry={retry} />;
  if (!data) return <p>Loading…</p>;
  return (
    <>
      <h1>Products</h1>
      <ul aria-label="Products">
        {data.map((p) => (
          <li key={p.id} className="product">
            <Link to={`/products/${p.id}`}>{p.name}</Link> {money(p.price)}
          </li>
        ))}
      </ul>
    </>
  );
}

function Product({ id, refreshCart }) {
  const { data: product, error, retry } = useLoad(() => api.product(id), [id]);
  const [live, setLive] = useState(null);
  const [added, setAdded] = useState(false);
  useEffect(
    () => livePrices((message) => String(message.id) === String(id) && setLive(message.price)),
    [id],
  );
  if (error instanceof ApiError && error.status === 404) {
    return (
      <>
        <h1>Product not found</h1>
        <Link to="/">Back to products</Link>
      </>
    );
  }
  if (error) return <ErrorPage error={error} retry={retry} />;
  if (!product) return <p>Loading…</p>;
  const add = async () => {
    await api.addToCart({ ...product, price: live ?? product.price });
    await refreshCart();
    setAdded(true);
  };
  return (
    <>
      <h1>{product.name}</h1>
      <p>{product.description}</p>
      <p>
        Price: <strong data-testid="price">{money(live ?? product.price)}</strong>
        {live !== null && " (live)"}
      </p>
      <button className="button button-primary" data-testid="add-to-cart" onClick={add}>
        Add to cart
      </button>
      {added && <p role="status">Added to cart</p>}
    </>
  );
}

function Search({ q }) {
  const { data, error, retry } = useLoad(() => api.search(q), [q]);
  if (error) return <ErrorPage error={error} retry={retry} />;
  if (!data) return <p>Searching…</p>;
  return (
    <>
      <h1>Results for “{q}”</h1>
      {data.results.length === 0 ? (
        <p>No products match “{q}”.</p>
      ) : (
        <ul aria-label="Results">
          {data.results.map((p) => (
            <li key={p.id}>
              <Link to={`/products/${p.id}`}>{p.name}</Link> {money(p.price)}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

function Login({ next, setUser }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState(null);
  const submit = async (event) => {
    event.preventDefault();
    setError(null);
    try {
      const { user } = await api.login(email, password);
      setUser(user);
      navigate(next ?? "/");
    } catch (e) {
      setError(e.message);
    }
  };
  return (
    <form onSubmit={submit}>
      <h1>Sign in</h1>
      <label>
        Email <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} />
      </label>
      <label>
        Password{" "}
        <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
      </label>
      {error && <p role="alert">{error}</p>}
      <button className="button button-primary" data-testid="sign-in">
        Sign in
      </button>
    </form>
  );
}

const total = (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0);

function Cart({ cart, refreshCart }) {
  const remove = async (item) => {
    await api.removeFromCart(item.id);
    await refreshCart();
  };
  return (
    <>
      <h1>Cart</h1>
      {cart.length === 0 ? (
        <p>Your cart is empty.</p>
      ) : (
        <>
          <ul aria-label="Cart items">
            {cart.map((item) => (
              <li key={item.id}>
                {item.name} {money(item.price)}{" "}
                <button
                  className="button button-small"
                  aria-label={`Remove ${item.name}`}
                  onClick={() => remove(item)}
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
          <p>Total: {money(total(cart))}</p>
          <Link to="/checkout">Check out</Link>
        </>
      )}
    </>
  );
}

function Checkout({ user, cart, refreshCart }) {
  const [placing, setPlacing] = useState(false);
  const [error, setError] = useState(null);
  const [card, setCard] = useState("4242 4242 4242 4242");
  // A charge that went through is kept, so trying again does not pay twice.
  const [charge, setCharge] = useState(null);
  if (!user) {
    return (
      <>
        <h1>Checkout</h1>
        <p>Sign in to check out.</p>
        <Link to="/login?next=/checkout">Sign in</Link>
      </>
    );
  }
  const place = async () => {
    setPlacing(true);
    setError(null);
    try {
      const paid = charge ?? (await payments.charge(total(cart), card));
      setCharge(paid);
      const order = await api.placeOrder(cart, total(cart));
      await Promise.all(cart.map((item) => api.removeFromCart(item.id)));
      await refreshCart();
      navigate(`/orders/${order.id}`);
    } catch (e) {
      setError(e.status === 503 ? "The shop is busy. Please try again." : e.message);
      setPlacing(false);
    }
  };
  return (
    <>
      <h1>Checkout</h1>
      <p>
        {cart.length} {cart.length === 1 ? "item" : "items"}, total {money(total(cart))}
      </p>
      <label>
        Card number{" "}
        <input
          inputMode="numeric"
          autoComplete="cc-number"
          value={card}
          onChange={(e) => setCard(e.target.value)}
          disabled={charge !== null}
        />
      </label>
      {error ? (
        <div role="alert">
          {error}{" "}
          <button className="button" onClick={place}>
            Try again
          </button>
        </div>
      ) : (
        <button
          className="button button-primary"
          data-testid="place-order"
          onClick={place}
          disabled={placing || cart.length === 0}
        >
          {placing ? "Placing order…" : "Place order"}
        </button>
      )}
    </>
  );
}

function Order({ id }) {
  const { data: order, error, retry } = useLoad(() => api.order(id), [id]);
  if (error) return <ErrorPage error={error} retry={retry} />;
  if (!order) return <p>Loading…</p>;
  return (
    <>
      <h1>Thank you!</h1>
      <p>
        Order {order.id} is confirmed. Total: {money(order.total)}
      </p>
      <Link to="/">Continue shopping</Link>
    </>
  );
}

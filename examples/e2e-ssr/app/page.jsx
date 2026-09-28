// A server component: its fetches run in the Next.js server, where no
// browser sees them. In the tests they go to `chrome-cdp mock serve`
// (mock_server in chrome-cdp.toml), and the page forwards the test's
// x-chrome-cdp-test header so each test gets its own mock state.
import { headers } from "next/headers";

export const dynamic = "force-dynamic";

async function api(path) {
  const test = (await headers()).get("x-chrome-cdp-test");
  const response = await fetch(`${process.env.API_URL}${path}`, {
    headers: test ? { "x-chrome-cdp-test": test } : {},
    cache: "no-store",
  });
  if (!response.ok) throw new Error(`GET ${path}: HTTP ${response.status}`);
  return response.json();
}

export default async function Home() {
  const [products, visits] = await Promise.all([api("/api/products"), api("/api/visits")]);
  return (
    <main>
      <h1>Products</h1>
      <ul aria-label="Products">
        {products.map((p) => (
          <li key={p.id}>
            {p.name} ${p.price.toFixed(2)}
          </li>
        ))}
      </ul>
      <p>Visit {visits.count}</p>
    </main>
  );
}

// A small client-side router: the History API and one path pattern per
// page, so the app needs no dependency beyond React.
import { useEffect, useState } from "react";

export function navigate(to) {
  history.pushState(null, "", to);
  dispatchEvent(new PopStateEvent("popstate"));
}

export function useLocation() {
  const [location, setLocation] = useState(() => new URL(window.location.href));
  useEffect(() => {
    const update = () => setLocation(new URL(window.location.href));
    addEventListener("popstate", update);
    return () => removeEventListener("popstate", update);
  }, []);
  return location;
}

/// `match("/products/:id", "/products/2")` is `{ id: "2" }`, else null.
export function match(pattern, path) {
  const want = pattern.split("/");
  const have = path.replace(/\/$/, "").split("/");
  if (want.length !== have.length) return null;
  const params = {};
  for (let i = 0; i < want.length; i++) {
    if (want[i].startsWith(":")) params[want[i].slice(1)] = decodeURIComponent(have[i]);
    else if (want[i] !== have[i]) return null;
  }
  return params;
}

export function Link({ to, children, ...props }) {
  const onClick = (event) => {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    navigate(to);
  };
  return (
    <a href={to} onClick={onClick} {...props}>
      {children}
    </a>
  );
}

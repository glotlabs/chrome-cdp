//#region src/api.ts
async function fetchCart() {
	return (await (await fetch("cart.json")).json()).items;
}
//#endregion
//#region src/cart.ts
var clicks = 0;
function countClick(what) {
	clicks += 1;
	return clicks;
}
function lineTotal(item, qty) {
	return item.price * qty;
}
function total(items, lines) {
	let sum = 0;
	for (const line of lines) {
		const item = items.find((i) => i.id === line.id);
		sum += lineTotal(item, line.qty);
	}
	return sum;
}
function checkoutLines(items) {
	return [...items.map((i) => ({
		id: i.id,
		qty: i.qty
	})), {
		id: 99,
		qty: 1
	}];
}
function renderTotal(items) {
	const line = document.createElement("p");
	line.dataset.testid = "total";
	line.textContent = `Total: ${total(items, items)}`;
	return line;
}
//#endregion
//#region src/components/CartItem.ts
function CartItem(item, actions) {
	const row = document.createElement("li");
	row.dataset.testid = `item-${item.id}`;
	const name = document.createElement("span");
	name.dataset.testid = `name-${item.id}`;
	name.textContent = item.name;
	const qty = document.createElement("span");
	qty.dataset.testid = `qty-${item.id}`;
	qty.textContent = String(item.qty);
	const add = document.createElement("button");
	add.dataset.testid = `add-${item.id}`;
	add.textContent = "Add";
	add.addEventListener("click", () => {
		countClick("add");
		actions.add(item.id);
	});
	const remove = document.createElement("button");
	remove.dataset.testid = `remove-${item.id}`;
	remove.textContent = "Remove";
	remove.addEventListener("click", () => {
		countClick("remove");
		actions.remove(item.id);
	});
	row.append(name, " × ", qty, " ", add, " ", remove);
	return row;
}
//#endregion
//#region src/components/CartList.ts
function CartList(items, actions) {
	const list = document.createElement("ul");
	list.dataset.testid = "cart-list";
	list.append(...items.map((item) => CartItem(item, actions)));
	return list;
}
//#endregion
//#region src/App.ts
function App(items, actions) {
	const app = document.createElement("main");
	app.dataset.testid = "cart";
	const title = document.createElement("h1");
	title.dataset.testid = "title";
	title.textContent = "Cart";
	const status = document.createElement("p");
	status.dataset.testid = "status";
	const checkout = document.createElement("button");
	checkout.dataset.testid = "checkout";
	checkout.textContent = "Checkout";
	checkout.addEventListener("click", () => {
		countClick("checkout");
		status.textContent = `Paid ${total(items, checkoutLines(items))}`;
	});
	app.append(title, CartList(items, actions), renderTotal(items), checkout, status);
	return app;
}
//#endregion
//#region \0vite/preload-helper.js
var scriptRel = /* @__PURE__ */ (function detectScriptRel() {
	const relList = typeof document !== "undefined" && document.createElement("link").relList;
	return relList && relList.supports && relList.supports("modulepreload") ? "modulepreload" : "preload";
})();
var assetsURL = function(dep, importerUrl) {
	return new URL(dep, importerUrl).href;
};
var seen = {};
var __vitePreload = function preload(baseModule, deps, importerUrl) {
	let promise = Promise.resolve();
	if (deps && deps.length > 0) {
		const links = document.getElementsByTagName("link");
		const cspNonceMeta = document.querySelector("meta[property=csp-nonce]");
		const cspNonce = cspNonceMeta?.nonce || cspNonceMeta?.getAttribute("nonce");
		function allSettled(promises) {
			return Promise.all(promises.map((p) => Promise.resolve(p).then((value) => ({
				status: "fulfilled",
				value
			}), (reason) => ({
				status: "rejected",
				reason
			}))));
		}
		function importMetaResolve(specifier) {
			if (import.meta.resolve) return import.meta.resolve(specifier);
			return new URL(
				specifier,
				/** #__KEEP__ */
				import.meta.url
			).href;
		}
		promise = allSettled(deps.map((dep) => {
			dep = assetsURL(dep, importerUrl);
			dep = importMetaResolve(dep);
			if (dep in seen) return;
			seen[dep] = true;
			const isCss = dep.endsWith(".css");
			for (let i = links.length - 1; i >= 0; i--) {
				const link = links[i];
				if (link.href === dep && (!isCss || link.rel === "stylesheet")) return;
			}
			const link = document.createElement("link");
			link.rel = isCss ? "stylesheet" : scriptRel;
			if (!isCss) link.as = "script";
			link.crossOrigin = "";
			link.href = dep;
			if (cspNonce) link.setAttribute("nonce", cspNonce);
			document.head.appendChild(link);
			if (isCss) return new Promise((res, rej) => {
				link.addEventListener("load", res);
				link.addEventListener("error", () => rej(/* @__PURE__ */ new Error(`Unable to preload CSS for ${dep}`)));
			});
		}).filter((p) => p !== void 0));
	}
	function handlePreloadError(err) {
		const e = new Event("vite:preloadError", { cancelable: true });
		e.payload = err;
		window.dispatchEvent(e);
		if (!e.defaultPrevented) throw err;
	}
	return promise.then((res) => {
		for (const item of res || []) {
			if (item.status !== "rejected") continue;
			handlePreloadError(item.reason);
		}
		return baseModule().catch(handlePreloadError);
	});
};
//#endregion
//#region src/main.ts
var root = document.getElementById("app");
var items = [];
function render() {
	root.replaceChildren(App(items, {
		add: async (id) => {
			items = items.map((i) => i.id === id ? {
				...i,
				qty: i.qty + 1
			} : i);
			render();
			const fresh = await fetchCart();
			items = items.map((i) => ({
				...i,
				price: fresh.find((f) => f.id === i.id)?.price ?? i.price
			}));
			render();
		},
		remove: (id) => {
			items = items.filter((i) => i.id !== id);
			render();
		}
	}));
}
render();
fetchCart().then((loaded) => {
	items = loaded;
	render();
});
window.addEventListener("later", async () => {
	const { later } = await __vitePreload(async () => {
		const { later } = await import("./later.js");
		return { later };
	}, [], import.meta.url);
	later("asked");
});
//#endregion

//# sourceMappingURL=index.js.map
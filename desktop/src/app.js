// Auth2API desktop frontend.
//
// No framework and no build step. Every command is a call into the Rust side
// and the UI is re-rendered from what comes back, so what the window shows
// and what the files say cannot drift apart.

const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);
const num = (n) => (n ?? 0).toLocaleString();

/// Tokens are the unit that matters here, and they run to seven digits - so
/// they are shown compact everywhere except tooltips, which keep the exact
/// figure.
function tok(n) {
  n = n ?? 0;
  if (n < 1000) return String(n);
  if (n < 1e6) return `${(n / 1e3).toFixed(n < 1e4 ? 1 : 0)}k`;
  return `${(n / 1e6).toFixed(2)}M`;
}

let state = null;
let expanded = false;

function el(tag, props = {}, children = []) {
  const node = Object.assign(document.createElement(tag), props);
  for (const child of [].concat(children)) if (child != null) node.append(child);
  return node;
}

function svg(path) {
  const node = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  node.setAttribute("viewBox", "0 0 24 24");
  node.innerHTML = path;
  return node;
}

/// The panel has no room for prose, so a failure shows as one line at the
/// bottom - still shown, never swallowed into the console.
function fail(error) {
  const node = $("error");
  if (!error) {
    node.hidden = true;
    return;
  }
  node.textContent = String(error).split("\n")[0];
  node.hidden = false;
}

async function call(command, args) {
  try {
    const result = await invoke(command, args);
    fail(null);
    return result;
  } catch (error) {
    fail(error);
    throw error;
  }
}

function flash(button) {
  button.classList.add("is-done");
  setTimeout(() => button.classList.remove("is-done"), 900);
}

function copy(text, button) {
  navigator.clipboard.writeText(text).then(() => flash(button));
}

// --- status ----------------------------------------------------------------

function render(next) {
  state = next;

  $("gate").hidden = next.signed_in;
  $("app").hidden = !next.signed_in;
  if (!next.signed_in) return;

  // The account has no column of its own; it hangs off the way out, which is
  // the only thing anyone does with it.
  const who = [next.account?.email, next.account?.plan, next.account?.account_id]
    .filter(Boolean)
    .join(" · ");
  $("btn-logout").title = who ? `Sign out\n${who}` : "Sign out";

  $("btn-power").classList.toggle("is-on", next.running);
  $("btn-power").title = next.running ? "Stop" : "Start";

  // Never overwrite a field mid-edit; the address stays editable while the
  // server runs, and committing a change restarts it on the new one.
  if (document.activeElement !== $("port")) $("port").value = next.port;

  renderScope(next.open_host, next);

  $("btn-url").title = next.running ? `Copy ${baseUrl(next)}` : "Start first";
  $("btn-url").disabled = !next.running;
}

/// `0.0.0.0` is what the socket binds, not an address anyone can dial, so a
/// shared server advertises a real interface instead.
function reachableHost(status, shared) {
  if (!shared) return "127.0.0.1";
  return status?.lan_addrs?.[0]?.ip || "0.0.0.0";
}

function baseUrl(status) {
  const host = reachableHost(status, status.open_host);
  return `http://${host}:${status.port}/v1`;
}

function renderScope(shared, status) {
  $("scope-local").classList.toggle("is-active", !shared);
  $("scope-net").classList.toggle("is-active", shared);

  const host = reachableHost(status, shared);
  $("host").textContent = host;
  $("host").classList.toggle("is-open", shared);

  const others = (status?.lan_addrs || []).slice(1);
  $("scope-net").title = shared
    ? status?.lan_addrs?.length
      ? `Shared on this network as ${host} (${status.lan_addrs[0].iface})` +
        (others.length ? `\nalso: ${others.map((a) => `${a.ip} (${a.iface})`).join(", ")}` : "")
      : "Shared, but no reachable network address was found"
    : "Share on this network - needs an API key";
}

async function refresh() {
  render(await call("status"));
}

// --- keys ------------------------------------------------------------------

function keyRow(key) {
  const copyBtn = el("button", {
    className: "icon",
    title: "Copy key",
    onclick: () => copy(key.secret, copyBtn),
  });
  copyBtn.append(svg('<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h8"/>'));

  const deleteBtn = el("button", {
    className: "icon icon-danger",
    title: "Delete key",
    onclick: async () => {
      await call("delete_key", { id: key.id });
      renderKeys();
      refresh();
    },
  });
  deleteBtn.append(svg('<path d="M4 7h16"/><path d="M9 7V5h6v2"/><path d="M6 7l1 13h10l1-13"/>'));

  const name = el("span", { className: "name", textContent: key.name, title: "Double-click to rename" });
  name.ondblclick = () => startRename(key, name);

  return el("div", { className: `key${key.revoked ? " is-revoked" : ""}` }, [
    name,
    el("span", { className: "abbr", textContent: key.masked.replace("sk-a2a-", "") }),
    el("span", {
      className: "tok",
      textContent: tok(key.total_tokens),
      title: `${num(key.total_tokens)} tokens · ${num(key.requests)} requests`,
    }),
    el("span", { className: "actions" }, [copyBtn, deleteBtn]),
  ]);
}

/// Swaps a name label for an input in place. Committing renames the key;
/// Escape or clicking away puts the label back untouched.
function startRename(key, label) {
  const input = el("input", { className: "rename", value: key.name });
  let settled = false;
  const done = async (commit) => {
    if (settled) return;
    settled = true;
    const name = input.value.trim();
    if (commit && name && name !== key.name) {
      await call("rename_key", { id: key.id, name });
    }
    renderKeys();
  };
  input.onkeydown = (e) => {
    if (e.key === "Enter") done(true);
    if (e.key === "Escape") done(false);
  };
  input.onblur = () => done(true);
  label.replaceWith(input);
  input.focus();
  input.select();
}

/// Adds an unnamed row at the top and focuses it, so creating a key and
/// naming it are one gesture rather than a form that lives on screen forever.
function startCreate() {
  const input = el("input", { className: "rename", placeholder: "name" });
  const row = el("div", { className: "key" }, [
    input,
    el("span", { className: "abbr" }),
    el("span", { className: "tok" }),
    el("span", { className: "actions" }),
  ]);
  let settled = false;
  const done = async (commit) => {
    if (settled) return;
    settled = true;
    if (commit) {
      await call("create_key", { name: input.value.trim() });
      await refresh();
    }
    renderKeys();
  };
  input.onkeydown = (e) => {
    if (e.key === "Enter") done(true);
    if (e.key === "Escape") done(false);
  };
  input.onblur = () => done(false);
  $("key-list").prepend(row);
  input.focus();
}

async function renderKeys() {
  const keys = await call("list_keys");
  $("key-list").replaceChildren(...keys.map(keyRow));
}

// --- stats -----------------------------------------------------------------

const PALETTE = ["--c1", "--c2", "--c3", "--c4", "--c5", "--c6"];

/// Draws one bucket per column with time on the x axis, oldest at the left.
/// `labelEvery` thins the axis so the ticks stay readable at any width.
function plot(target, axisTarget, buckets, labelOf, labelEvery, hue) {
  $(target).style.setProperty("--bar", `var(${hue})`);
  const max = Math.max(0, ...buckets.map((b) => b.total_tokens));
  $(target).replaceChildren(
    ...buckets.map((bucket) => {
      const bar = el("div", { className: "bar" });
      bar.style.height = max === 0 ? "0%" : `${(bucket.total_tokens / max) * 100}%`;
      if (bucket.total_tokens > 0) bar.classList.add("tiny");
      return el(
        "div",
        {
          className: "col",
          title: `${labelOf(bucket)} · ${num(bucket.total_tokens)} tokens · ${num(bucket.requests)} req`,
        },
        bar
      );
    })
  );
  $(axisTarget).replaceChildren(
    ...buckets.map((bucket, i) =>
      el("span", { textContent: i % labelEvery === 0 ? labelOf(bucket) : "" })
    )
  );
}

function tile(k, v, sub, title) {
  return el("div", { className: "tile", title: title || "" }, [
    el("div", { className: "k", textContent: k }),
    el("div", { className: "v", textContent: v }),
    el("div", { className: "s", textContent: sub || "" }),
  ]);
}

function split(target, heading, buckets, labelOf) {
  const rows = [...buckets]
    .sort((a, b) => b.total_tokens - a.total_tokens)
    .slice(0, 6)
    .map((bucket, i) => {
      const swatch = el("span", { className: "swatch" });
      swatch.style.setProperty("--bar", `var(${PALETTE[i % PALETTE.length]})`);
      return el("div", { className: "line", title: `${num(bucket.total_tokens)} tokens` }, [
        swatch,
        el("b", { textContent: labelOf(bucket) }),
        el("span", { textContent: tok(bucket.total_tokens) }),
      ]);
    });
  $(target).replaceChildren(el("h3", { textContent: heading }), ...rows);
}

async function renderStats() {
  const hours = $("window").value;
  const report = await call("usage", { hours: hours ? Number(hours) : null });
  const t = report.totals;

  $("tiles").replaceChildren(
    tile("tokens", tok(t.total_tokens), num(t.total_tokens)),
    tile("in", tok(t.prompt_tokens), `${tok(t.cached_tokens)} cached`),
    tile("out", tok(t.completion_tokens), `${tok(t.reasoning_tokens)} reasoning`),
    tile(
      "usd",
      t.estimated_cost_usd == null ? "—" : `$${t.estimated_cost_usd.toFixed(2)}`,
      `${num(t.requests)} req`,
      // A subscription bills a flat monthly fee, so this is a comparison the
      // user opted into by pricing models in config.toml - never a charge.
      t.estimated_cost_usd == null
        ? "No prices set in config.toml"
        : "Equivalent list price, not a charge"
    )
  );

  const days = report.by_day.slice(-30);
  plot("chart-days", "axis-days", days, (b) => b.key.slice(5), days.length > 14 ? 5 : 2, "--c1");
  plot("chart-hours", "axis-hours", report.by_hour_of_day, (b) => `${b.key}`, 3, "--c2");

  split("split-keys", "by key", report.by_key, (b) => b.label || b.key);
  split("split-models", "by model", report.by_model, (b) => b.key);
}

// --- wiring ----------------------------------------------------------------

$("btn-login").onclick = async (e) => {
  e.target.disabled = true;
  try {
    await call("login");
    await refresh();
    await renderKeys();
  } finally {
    e.target.disabled = false;
  }
};

$("btn-logout").onclick = async () => render(await call("logout"));

async function startServer() {
  render(
    await call("start", {
      port: Number($("port").value),
      open: $("scope-net").classList.contains("is-active"),
    })
  );
}

$("btn-power").onclick = async (e) => {
  e.currentTarget.disabled = true;
  try {
    if (state?.running) render(await call("stop"));
    else await startServer();
  } catch {
    // The message is already on screen; leave the button usable to retry.
  } finally {
    e.currentTarget.disabled = false;
  }
};

/// Applies an address change. A running server is rebound rather than left on
/// the old address, because a field that accepts an edit and then ignores it
/// is worse than one that refuses it.
async function applyAddress() {
  if (!state?.running) return;
  try {
    await call("stop");
    await startServer();
  } catch {
    // `start` refuses to expose the subscription with no API key. The reason
    // is on screen; reflect that the server is now stopped.
    await refresh();
  }
}

async function setScope(shared) {
  renderScope(shared, state);
  await applyAddress();
}

$("scope-local").onclick = () => setScope(false);
$("scope-net").onclick = () => setScope(true);

$("btn-url").onclick = (e) => {
  if (state?.running) copy(`http://${state.address}/v1`, e.currentTarget);
};

$("port").onchange = async () => {
  if (state?.running) return applyAddress();
  await call("save_settings", { port: Number($("port").value) });
};

$("btn-new-key").onclick = startCreate;

$("btn-expand").onclick = async (e) => {
  expanded = !expanded;
  e.currentTarget.classList.toggle("is-open", expanded);
  $("stats").hidden = !expanded;
  $("app").classList.toggle("is-expanded", expanded);
  await call("set_expanded", { expanded });
  if (expanded) renderStats();
};

$("window").onchange = renderStats;

$("btn-reset").onclick = async () => {
  await call("usage_reset");
  renderStats();
  renderKeys();
};

refresh().then(() => state?.signed_in && renderKeys());

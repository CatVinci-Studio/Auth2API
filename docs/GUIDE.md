<p align="center">
  <img src="../desktop/src-tauri/icons/icon.png" alt="Auth2API" width="120" height="120" />
</p>

<h1 align="center">Auth2API</h1>

<p align="center">
  <strong>Sign in with a ChatGPT account. Get an OpenAI-compatible API on localhost.</strong><br>
  Requests spend your ChatGPT subscription, not an API-key balance.
</p>

<p align="center">
  <a href="https://github.com/CatVinci-Studio/Auth2API/releases/latest"><strong>Download</strong></a> ·
  <a href="./GUIDE.zh.md">中文</a>
</p>

<p align="center">
  <a href="https://github.com/CatVinci-Studio/Auth2API/releases/latest"><img alt="version" src="https://img.shields.io/github/v/release/CatVinci-Studio/Auth2API"></a>
  <img alt="platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey">
  <a href="../LICENSE"><img alt="license" src="https://img.shields.io/badge/license-MIT-yellow"></a>
</p>

---

## What it is

Auth2API signs in to your ChatGPT account through the browser and serves that
session as an OpenAI-compatible API on `127.0.0.1`. Anything that speaks the
OpenAI protocol — an editor plugin, an SDK, a script — points at it and works,
while the tokens come out of your subscription instead of an API-key bill.

Ships as one binary and one small desktop app, both on the same core.

```
┌──────────────┐   PKCE OAuth    ┌──────────────┐
│  Auth2API    │ ──────────────► │ auth.openai  │
│              │ ◄────────────── │    .com      │
│  127.0.0.1   │   access token  └──────────────┘
│    :8787     │
│              │  Responses SSE  ┌──────────────┐
│  /v1/chat/…  │ ──────────────► │  chatgpt.com │
│  /v1/respon… │ ◄────────────── │ /backend-api │
└──────▲───────┘                 └──────────────┘
       │ OpenAI-shaped HTTP + your own API keys
   your clients
```

## How it works, and what that means

The login reuses the OpenAI Codex CLI's own public OAuth client id. That is
what makes the resulting token spend a ChatGPT subscription: it is only
accepted by `chatgpt.com/backend-api`, never by `api.openai.com`.

Consequences worth knowing before you rely on this:

- **Unofficial.** OpenAI has not published this as an integration point. A
  changed client id, a checked `originator`, or a renamed model can break it
  without notice. Read your plan's terms and decide for yourself.
- **Only some models work.** The ChatGPT backend rejects anything outside a
  short list with `400 ... not supported when using Codex with a ChatGPT
  account`. Auth2API falls back to a working model instead of passing that
  400 on — see [Models](#models).
- **Streaming only, upstream.** The backend refuses `stream: false`. A
  non-streaming request is served by consuming the SSE here and assembling
  the body, never by asking upstream for one.
- **Sampling knobs are dropped.** `temperature`, `top_p` and friends are hard
  400s upstream, so they are removed rather than forwarded — a client that
  sets a harmless default should not be punished for it.

## Install

Grab a build from [Releases](https://github.com/CatVinci-Studio/Auth2API/releases/latest):

| Platform | Desktop app | Headless CLI |
|---|---|---|
| macOS (Apple Silicon) | `Auth2API_0.1.0_aarch64.dmg` | `auth2api-macos-arm64` |
| macOS (Intel) | `Auth2API_0.1.0_x64.dmg` | `auth2api-macos-x64` |
| Windows | `Auth2API_0.1.0_x64-setup.exe` · `_x64_en-US.msi` | `auth2api-windows-x64.exe` |
| Linux | `Auth2API_0.1.0_amd64.AppImage` · `_amd64.deb` | `auth2api-linux-x64` |

Or build it yourself — Rust is the only prerequisite for the CLI:

```bash
cargo build --release              # target/release/auth2api
cd desktop && cargo tauri build    # the desktop bundle
```

## Quick start

```bash
auth2api login          # opens your browser
auth2api keys new zed   # prints a key; copy it
auth2api serve          # http://127.0.0.1:8787/v1
```

Point anything OpenAI-compatible at it:

```bash
export OPENAI_BASE_URL=http://127.0.0.1:8787/v1
export OPENAI_API_KEY=sk-a2a-...
```

The desktop app does the same things in a small panel: the icon is the on/off
switch, the row it sits in is the address, and the rest is the key list. The
chevron on the right edge widens the window into the usage charts. It drives
the same core and reads the same files as the CLI, so the two can never
disagree.

## Endpoints

| | |
|---|---|
| `POST /v1/chat/completions` | Translated to/from the Responses API. Streaming, tool calling, and images all supported. |
| `POST /v1/responses` | Passed through essentially as written, for native Responses clients. |
| `GET /v1/models` | The models this login can actually serve. |
| `GET /v1/usage?hours=N` | Auth2API's own accounting (not an OpenAI route). |
| `GET /health` | Login and server state. |

Tool calling works on both. The two formats express it differently —
`tools[].function.{…}` versus flat `tools[].{…}`, `message.tool_calls[]`
versus `function_call` output items, `{role:"tool"}` versus
`function_call_output` — and `crates/auth2api-core/src/translate.rs` maps
between them in both directions, including the streaming case where argument
deltas arrive keyed by item id and have to be re-keyed to a positional index.

## API keys

Keys are the credential *your clients* present to Auth2API. They are separate
from the ChatGPT login, and there can be several so that each client gets its
own — which is what makes the per-key usage meaningful.

```bash
auth2api keys new phone      # mint one
auth2api keys list           # names, masked secrets, tokens used
auth2api keys show k_ab12    # print a secret again
auth2api keys revoke k_ab12  # stop it working, keep its history
auth2api keys delete k_ab12  # remove it outright
```

Two behaviours that are deliberate:

- With **no keys at all**, the server serves any caller that can reach it.
  That is the zero-setup loopback case, and `serve` refuses to bind anything
  but loopback in that state.
- **Revoking your last key does not reopen the server.** Once keys exist the
  door stays shut; only deleting them all returns to the open state, which is
  an explicit act rather than a side effect.

## Usage and cost

```bash
auth2api stats                # all time
auth2api stats --hours 24     # last day
auth2api stats --json         # the same report, machine-readable
```

Every completed request appends one line to `usage.jsonl`: timestamp, model,
tokens in/out/cached/reasoning, duration, success, and which key made it. The
reports are all slices of that log — per model, per key, per hour of day, per
day.

**On money.** A ChatGPT subscription is a flat monthly fee, so there is no
per-request charge to report and Auth2API cannot observe one. What
`estimated_cost_usd` shows is the *equivalent list price* — what these same
tokens would have cost through an API key — which is the number that tells you
whether the subscription is paying for itself. It is empty until you supply
prices yourself:

```toml
[pricing."gpt-5.6-luna"]
input = 1.25          # USD per 1M tokens
cached_input = 0.125
output = 10.0
```

No prices ship by default, on purpose: invented defaults would produce
authoritative-looking figures that are wrong the moment a price changes.

## Sharing on a local network

Click the host in the address bar (or `auth2api serve --host 0.0.0.0`) and
other machines can reach it. Two things happen automatically:

- **An API key is required.** Binding off-loopback without one is refused, not
  warned about — in that state the server is an open relay for your
  subscription.
- **The address bar shows a dialable address**, not `0.0.0.0`. It comes from
  enumerating interfaces and skipping tunnels (`utun`, `wg`, `ipsec`, …),
  because with a VPN up the obvious shortcuts — asking the routing table, or
  preferring a private range — both return the tunnel address, which is
  precisely the one nobody on your network can reach.

Check what "local network" means on your connection before relying on it. On a
home router it is your own devices; on a university or office network the same
subnet can be hundreds of machines.

## Models

The ChatGPT backend only accepts a short list of models. The default list
lives in `config.toml` rather than in the binary, so a rename upstream is a
one-line fix instead of a release:

```toml
default_model = "gpt-5.6-luna"
models = ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]

# Reach the server from a client with another model hard-coded into it.
[model_aliases]
"gpt-4o" = "gpt-5.6-terra"
```

A request for an unusable model falls back to `default_model` with a warning
in the log, because the common case is a client with an unrelated default and
a silent 400 from a backend the user never chose to talk to is a much worse
first experience than an answer.

## Configuration

`config.toml`, `auth.json`, `keys.json` and `usage.jsonl` live in the platform
config directory (`auth2api config path` prints it). Set `AUTH2API_HOME` to put
them somewhere else — handy for keeping a separate account per project.

| Key | Default | |
|---|---|---|
| `host` | `127.0.0.1` | `0.0.0.0` also requires an API key |
| `port` | `8787` | |
| `proxy` | *(none)* | `http://`, `https://`, `socks5://` |
| `default_model` | `gpt-5.6-luna` | used when a request names an unusable model |
| `models` | see above | also what `/v1/models` advertises |
| `model_aliases` | *(none)* | client model → upstream model |
| `default_instructions` | *(generic)* | used when a request has no system message |
| `pricing` | *(none)* | see [Usage and cost](#usage-and-cost) |
| `api_key` | *(none)* | legacy single key; `keys.json` is the current mechanism |

## Layout

```
crates/auth2api-core/     login, HTTP surface, translation, accounting
  src/auth/               PKCE OAuth, credential file, token refresh
  src/upstream/           the ChatGPT backend client and its SSE stream
  src/api/                the /v1 routes
  src/translate.rs        Chat Completions <-> Responses
  src/keys.rs             local API keys
  src/stats.rs            the usage log and its reports
crates/auth2api-cli/      the `auth2api` binary
desktop/                  Tauri app (plain HTML/CSS/JS, no build step)
```

## Security notes

- `auth.json` and `keys.json` are written `0600`. They hold a live refresh
  token for your ChatGPT account and every client secret respectively.
- The usage log records key *ids and names*, never secrets — it is the file
  you would paste into a bug report.
- Binding off-loopback without a key is refused, not warned about: this
  process spends your subscription, and in that state it is an open relay
  for it.
- Key comparison is length-then-constant-time, so a prefix of a real key
  cannot be found by timing.

## Tests

```bash
cargo test --workspace
```

Unit tests cover the translation in both directions, the streaming
tool-call re-keying, key authentication, and the accounting. An integration
test drives the real router end to end — routes, key enforcement, error
envelopes, and per-key attribution — against a temporary `AUTH2API_HOME`, so
it never touches your own state.

What the tests do **not** cover is the upstream call itself: exercising
`chatgpt.com/backend-api` needs a real ChatGPT login, which CI cannot have.
That leg was verified by hand against a live account — non-streaming,
streaming with a usage chunk, and a tool call streaming its arguments through
to `finish_reason: "tool_calls"` — but nothing re-checks it automatically, so
treat an upstream change as something the suite will not catch for you.

## Releasing

Pushing a `v*` tag builds every platform and publishes a draft release with
the desktop bundles and the CLI binaries attached:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

Keep `version` in the workspace `Cargo.toml` and in
`desktop/src-tauri/tauri.conf.json` in step with the tag — nothing checks that
they agree, and a bundle that disagrees with its tag is confusing to everyone
who downloads it.

## License

MIT.

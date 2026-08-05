<p align="center">
  <img src="desktop/src-tauri/icons/icon.png" alt="Auth2API" width="120" height="120" />
</p>

<h1 align="center">Auth2API</h1>

<p align="center">
  <strong>A thin shell that turns a ChatGPT subscription into an OpenAI-compatible API.</strong><br>
  Sign in with your account, get <code>http://127.0.0.1:8787/v1</code>. Tokens come out of the
  subscription, not an API-key bill.
</p>

<p align="center">
  <a href="https://github.com/CatVinci-Studio/Auth2API/releases/latest"><strong>Download</strong></a> ·
  <a href="./docs/GUIDE.md">Docs</a> ·
  <a href="./README.zh.md">中文</a>
</p>

<p align="center">
  <a href="https://github.com/CatVinci-Studio/Auth2API/releases/latest"><img alt="version" src="https://img.shields.io/github/v/release/CatVinci-Studio/Auth2API"></a>
  <img alt="platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey">
  <a href="./LICENSE"><img alt="license" src="https://img.shields.io/badge/license-MIT-yellow"></a>
</p>

---

Comes two ways, on the same core:

- **Desktop app** — a small panel: sign in, start the server, manage keys, watch usage.
- **CLI** — `auth2api` for headless use on a server or in a script.

## Download

From [Releases](https://github.com/CatVinci-Studio/Auth2API/releases/latest):

| Platform | Desktop app | CLI |
|---|---|---|
| macOS (Apple Silicon) | `Auth2API_0.1.0_aarch64.dmg` | `auth2api-macos-arm64` |
| macOS (Intel) | `Auth2API_0.1.0_x64.dmg` | `auth2api-macos-x64` |
| Windows | `Auth2API_0.1.0_x64-setup.exe` | `auth2api-windows-x64.exe` |
| Linux | `Auth2API_0.1.0_amd64.AppImage` · `_amd64.deb` | `auth2api-linux-x64` |

## Use

```bash
auth2api login          # opens your browser
auth2api keys new zed   # prints a key; copy it
auth2api serve          # http://127.0.0.1:8787/v1
```

Then point anything OpenAI-compatible at it:

```bash
export OPENAI_BASE_URL=http://127.0.0.1:8787/v1
export OPENAI_API_KEY=sk-a2a-...
```

The desktop app does the same in a window.

> **Unofficial.** The login reuses the Codex CLI's public OAuth client id, which
> is what makes the token spend a subscription. OpenAI has not published this as
> an integration point and can break it at any time. Only a few models work, and
> "cost" in the usage view is an equivalent list price, never a real charge.
> Details in the [docs](./docs/GUIDE.md).

## License

MIT.

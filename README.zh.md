<p align="center">
  <img src="desktop/src-tauri/icons/icon.png" alt="Auth2API" width="120" height="120" />
</p>

<h1 align="center">Auth2API</h1>

<p align="center">
  <strong>把 ChatGPT 订阅转成 OpenAI 兼容 API 的一层薄壳。</strong><br>
  用账号登录，得到 <code>http://127.0.0.1:8787/v1</code>。token 从订阅里出，不产生 API key 账单。
</p>

<p align="center">
  <a href="https://github.com/CatVinci-Studio/Auth2API/releases/latest"><strong>下载</strong></a> ·
  <a href="./docs/GUIDE.zh.md">文档</a> ·
  <a href="./README.md">English</a>
</p>

<p align="center">
  <a href="https://github.com/CatVinci-Studio/Auth2API/releases/latest"><img alt="version" src="https://img.shields.io/github/v/release/CatVinci-Studio/Auth2API"></a>
  <img alt="platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey">
  <a href="./LICENSE"><img alt="license" src="https://img.shields.io/badge/license-MIT-yellow"></a>
</p>

---

两种用法，同一份核心：

- **桌面端** —— 一个小面板：登录、开关服务、管理 key、看用量。
- **命令行** —— `auth2api`，适合放在服务器上或写进脚本。

## 下载

从 [Releases](https://github.com/CatVinci-Studio/Auth2API/releases/latest) 获取：

| 平台 | 桌面端 | 命令行 |
|---|---|---|
| macOS (Apple Silicon) | `Auth2API_0.1.0_aarch64.dmg` | `auth2api-macos-arm64` |
| macOS (Intel) | `Auth2API_0.1.0_x64.dmg` | `auth2api-macos-x64` |
| Windows | `Auth2API_0.1.0_x64-setup.exe` | `auth2api-windows-x64.exe` |
| Linux | `Auth2API_0.1.0_amd64.AppImage` · `_amd64.deb` | `auth2api-linux-x64` |

## 用法

```bash
auth2api login          # 打开浏览器登录
auth2api keys new zed   # 生成 key，复制它
auth2api serve          # http://127.0.0.1:8787/v1
```

然后把任何 OpenAI 兼容的客户端指过来：

```bash
export OPENAI_BASE_URL=http://127.0.0.1:8787/v1
export OPENAI_API_KEY=sk-a2a-...
```

桌面端在窗口里做同样的事。

## 许可

MIT。

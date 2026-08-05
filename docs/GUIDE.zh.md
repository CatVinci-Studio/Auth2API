<p align="center">
  <img src="../desktop/src-tauri/icons/icon.png" alt="Auth2API" width="120" height="120" />
</p>

<h1 align="center">Auth2API</h1>

<p align="center">
  <strong>用 ChatGPT 账号登录，在本机得到一个 OpenAI 兼容的 API。</strong><br>
  请求消耗的是你的 ChatGPT 订阅额度，不是 API key 余额。
</p>

<p align="center">
  <a href="https://github.com/CatVinci-Studio/Auth2API/releases/latest"><strong>下载</strong></a> ·
  <a href="./GUIDE.md">English</a>
</p>

<p align="center">
  <a href="https://github.com/CatVinci-Studio/Auth2API/releases/latest"><img alt="version" src="https://img.shields.io/github/v/release/CatVinci-Studio/Auth2API"></a>
  <img alt="platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey">
  <a href="../LICENSE"><img alt="license" src="https://img.shields.io/badge/license-MIT-yellow"></a>
</p>

---

## 这是什么

Auth2API 通过浏览器登录你的 ChatGPT 账号，然后把这个会话在 `127.0.0.1` 上暴露成一个
OpenAI 兼容的 API。任何说 OpenAI 协议的东西——编辑器插件、SDK、脚本——指过来就能用，
而 token 从订阅里出，不产生 API key 账单。

一个命令行二进制 + 一个小桌面端，跑的是同一份核心。

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
       │ OpenAI 格式的 HTTP + 你自己的 API key
     你的客户端
```

## 原理，以及这意味着什么

登录复用的是 OpenAI Codex CLI 自己的公开 OAuth client id。这正是拿到的 token 能花订阅
额度的原因：它只被 `chatgpt.com/backend-api` 接受，`api.openai.com` 一律不认。

在依赖它之前，有几件事你应该知道：

- **非官方。** OpenAI 从未把这条路作为对外的接入点。换掉 client id、开始校验
  `originator`、或者重命名模型，任何一件都能让它随时失效。请自行阅读你所在套餐的条款
  并做判断。
- **只有部分模型可用。** ChatGPT 后端对名单外的模型一律返回
  `400 ... not supported when using Codex with a ChatGPT account`。Auth2API 会回退到一个
  可用模型，而不是把这个 400 原样丢给你——见 [模型](#模型)。
- **上游只有流式。** 该后端拒绝 `stream: false`。非流式请求是在本地消费完 SSE 后重新
  组装出来的，而不是向上游要一个完整响应体。
- **采样参数被丢弃。** `temperature`、`top_p` 这类参数在上游是硬性 400，所以它们被移除
  而非透传——客户端设了一个无害的默认值，不该因此被惩罚。

## 安装

从 [Releases](https://github.com/CatVinci-Studio/Auth2API/releases/latest) 下载：

| 平台 | 桌面端 | 无头 CLI |
|---|---|---|
| macOS (Apple Silicon) | `Auth2API_0.1.0_aarch64.dmg` | `auth2api-macos-arm64` |
| macOS (Intel) | `Auth2API_0.1.0_x64.dmg` | `auth2api-macos-x64` |
| Windows | `Auth2API_0.1.0_x64-setup.exe` · `_x64_en-US.msi` | `auth2api-windows-x64.exe` |
| Linux | `Auth2API_0.1.0_amd64.AppImage` · `_amd64.deb` | `auth2api-linux-x64` |

也可以自己编译，CLI 只需要 Rust：

```bash
cargo build --release              # target/release/auth2api
cd desktop && cargo tauri build    # 桌面端安装包
```

## 快速开始

```bash
auth2api login          # 打开浏览器登录
auth2api keys new zed   # 生成一个 key，复制它
auth2api serve          # http://127.0.0.1:8787/v1
```

然后把任何 OpenAI 兼容的客户端指过来：

```bash
export OPENAI_BASE_URL=http://127.0.0.1:8787/v1
export OPENAI_API_KEY=sk-a2a-...
```

桌面端在一个小面板里做同样的事：图标本身就是开关，它所在那一行是地址，剩下的是 key
列表。右侧边缘的箭头把窗口展开成用量图表。它和 CLI 跑同一份核心、读同一批文件，所以
两边不可能对不上。

## 接口

| | |
|---|---|
| `POST /v1/chat/completions` | 与 Responses API 双向翻译。流式、工具调用、图片都支持。 |
| `POST /v1/responses` | 基本原样透传，给原生 Responses 客户端用。 |
| `GET /v1/models` | 这个登录实际能用的模型。 |
| `GET /v1/usage?hours=N` | Auth2API 自己的用量统计（不是 OpenAI 的路由）。 |
| `GET /health` | 登录状态和服务状态。 |

两个接口都支持工具调用。两种格式表达同一件事的方式不同——`tools[].function.{…}` 对
扁平的 `tools[].{…}`、`message.tool_calls[]` 对 `function_call` 输出项、`{role:"tool"}` 对
`function_call_output`——`crates/auth2api-core/src/translate.rs` 负责双向映射，包括流式那
一路：参数增量是按 item id 下发的，必须重新映射成位置索引。

## API key

这里的 key 是**你的客户端**用来访问 Auth2API 的凭证，和 ChatGPT 登录是两回事。可以同时
存在多个，让每个客户端各用一个——这正是按 key 统计用量有意义的前提。

```bash
auth2api keys new phone      # 新建
auth2api keys list           # 名称、打码后的密钥、已用 token
auth2api keys show k_ab12    # 再次打印完整密钥
auth2api keys revoke k_ab12  # 停用，但保留历史用量
auth2api keys delete k_ab12  # 彻底删除
```

两个刻意的行为：

- **一个 key 都没有时**，服务接受任何能连上它的调用方。这是零配置的本机场景，此时它会
  拒绝绑定到 loopback 以外的任何地址。
- **撤销最后一个 key 不会让服务重新敞开。** 只要 key 存在过，门就一直关着；只有把它们
  全部删除才回到开放状态，而删除是一个明确的动作，不是某个操作的副作用。

## 用量与花费

```bash
auth2api stats                # 全部
auth2api stats --hours 24     # 最近一天
auth2api stats --json         # 同一份报告，机器可读
```

每次请求完成后往 `usage.jsonl` 追加一行：时间戳、模型、输入/输出/缓存/推理 token、耗时、
成败、以及由哪个 key 发起。所有报表都是这份日志的切片——按模型、按 key、按小时、按天。

**关于钱。** ChatGPT 订阅是固定月费，因此不存在按次计费，Auth2API 也观察不到任何扣费。
`estimated_cost_usd` 显示的是**等价目录价**——这些 token 如果走 API key 要花多少钱——这个
数用来判断订阅有没有回本。它默认是空的，需要你自己填价格：

```toml
[pricing."gpt-5.6-luna"]
input = 1.25          # 每 100 万 token 的美元价
cached_input = 0.125
output = 10.0
```

默认不附带任何价格是刻意的：编一组默认值只会产出看起来很权威、但价格一变就错的数字。

## 局域网共享

在应用里把地址切到网络模式，或者运行 `auth2api serve --host 0.0.0.0`，其他机器就能访问。
有两件事会自动发生：

- **必须有 API key。** 没有 key 时绑定到非 loopback 地址会被直接拒绝，而不是给个警告
  ——那种状态下这个服务就是你订阅额度的开放中继。
- **地址栏显示的是能拨通的地址**，不是 `0.0.0.0`。它来自枚举网卡并跳过隧道接口
  （`utun`、`wg`、`ipsec` 等），因为在开着 VPN 时，两种想当然的做法——问路由表、或者优先
  选私有网段——返回的都是隧道地址，而那恰恰是局域网里没人能连上的那个。

依赖它之前先确认你这条网络上"局域网"到底意味着什么。家用路由器下是你自己的几台设备；
在学校或公司网络里，同一个子网可能有几百台机器。

## 模型

ChatGPT 后端只接受一个很短的模型名单。默认名单放在 `config.toml` 而不是写死在二进制里，
这样上游改名时改一行就行，不用等一个新版本：

```toml
default_model = "gpt-5.6-luna"
models = ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]

# 让写死了别的模型名的客户端也能连上。
[model_aliases]
"gpt-4o" = "gpt-5.6-terra"
```

请求了不可用的模型会回退到 `default_model` 并在日志里留一条警告。因为最常见的情况是
客户端里带着一个不相干的默认值，而让用户从一个他从未主动选择过的后端收到一个无声的
400，比直接给他一个回答要糟糕得多。

## 配置

`config.toml`、`auth.json`、`keys.json`、`usage.jsonl` 位于系统配置目录
（`auth2api config path` 会打印路径）。设置 `AUTH2API_HOME` 可以换到别处——想给每个项目
用不同账号时很方便。

| 键 | 默认值 | |
|---|---|---|
| `host` | `127.0.0.1` | 用 `0.0.0.0` 还需要至少一个 API key |
| `port` | `8787` | |
| `proxy` | *(无)* | `http://`、`https://`、`socks5://` |
| `default_model` | `gpt-5.6-luna` | 请求了不可用模型时使用 |
| `models` | 见上文 | 同时也是 `/v1/models` 对外公布的列表 |
| `model_aliases` | *(无)* | 客户端模型名 → 上游模型名 |
| `default_instructions` | *(通用)* | 请求里没有 system 消息时使用 |
| `pricing` | *(无)* | 见[用量与花费](#用量与花费) |
| `api_key` | *(无)* | 遗留的单 key 方式；现在用 `keys.json` |

## 目录结构

```
crates/auth2api-core/     登录、HTTP 接口、格式翻译、用量统计
  src/auth/               PKCE OAuth、凭证文件、token 刷新
  src/upstream/           ChatGPT 后端客户端及其 SSE 流
  src/api/                /v1 路由
  src/translate.rs        Chat Completions <-> Responses
  src/keys.rs             本地 API key
  src/stats.rs            用量日志与报表
crates/auth2api-cli/      auth2api 二进制
desktop/                  Tauri 应用（纯 HTML/CSS/JS，无构建步骤）
```

## 安全说明

- `auth.json` 和 `keys.json` 以 `0600` 写入。前者持有你 ChatGPT 账号的有效 refresh token，
  后者持有全部客户端密钥。
- 用量日志只记录 key 的 **id 和名称**，从不记录密钥本身——它是你会贴进 issue 的那个文件。
- 没有 key 时绑定非 loopback 地址是直接拒绝而非警告：这个进程花的是你的订阅额度，那种
  状态下它就是一个开放中继。
- key 比对先比长度再做常数时间比较，因此无法通过计时试出真实 key 的前缀。

## 测试

```bash
cargo test --workspace
```

单元测试覆盖双向翻译、流式工具调用的索引重映射、key 鉴权和用量统计。集成测试驱动真实
路由端到端跑一遍——路由、key 校验、错误信封、按 key 归属——运行在临时的 `AUTH2API_HOME`
下，不会碰到你自己的状态。

测试**没有**覆盖的是上游调用本身：要打通 `chatgpt.com/backend-api` 需要一个真实的
ChatGPT 登录，而 CI 拿不到。这一段是对着真实账号手工验证过的——非流式、带 usage chunk 的
流式、以及工具调用把参数流式送到 `finish_reason: "tool_calls"`——但没有任何自动化会复查
它，所以上游一旦变化，测试套件不会替你发现。

## 发布

推一个 `v*` 标签会构建全部平台并自动发布 release，附带桌面端安装包和各平台的 CLI
二进制：

```bash
git tag v0.1.0 && git push origin v0.1.0
```

release 在所有平台上传完之前保持草稿状态，全部成功后自动转为正式发布。只要有一个平台
失败就停在草稿——那是该去排查的状态，不是该发出去的状态。

记得让工作区 `Cargo.toml` 和 `desktop/src-tauri/tauri.conf.json` 里的 `version` 与标签保持
一致——没有任何东西会检查它们是否吻合，而一个和标签对不上的安装包会让所有下载的人困惑。

## 许可

MIT。

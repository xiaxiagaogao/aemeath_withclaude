# Aemeath MCP & HTTP API 文档

本文档详细说明 Aemeath 桌宠的 MCP 工具、HTTP API 以及双向交互机制。

---

## 架构概览

```
Claude Code
  ├── HTTP hooks → POST :9527/api/hook/*
  └── MCP Client → :9528/mcp (JSON-RPC)

Aemeath Pet (Tauri Desktop App)
  ├── HTTP Server (:9527)   → 接收 hook 推送 + 前端轮询
  ├── MCP Server (:9528)    → 富交互（tools / resources）
  ├── State Manager (Rust)  → 状态机 + 气泡锁
  └── WebView Frontend      → CSS sprite 动画 + 气泡 + 输入 UI
```

---

## MCP Tools (端口 9528)

### 连接配置

```json
{
  "aemeath": {
    "type": "http",
    "url": "http://127.0.0.1:9528/mcp"
  }
}
```

### 工具列表

#### `aemeath_show` — 显示自定义气泡

在宠物上方显示自定义消息气泡。

**参数：**
| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `msg` | string | ✓ | 要显示的消息内容 |

**示例：**
```json
{
  "name": "aemeath_show",
  "arguments": {
    "msg": "正在编译项目..."
  }
}
```

---

#### `aemeath_ask` — 提问（非阻塞）

通过宠物 UI 向用户展示问题（仅显示，不等待回答）。

**参数：**
| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `question` | string | ✓ | 问题内容 |
| `options` | string[] | 可选 | 选项列表（仅展示） |

**示例：**
```json
{
  "name": "aemeath_ask",
  "arguments": {
    "question": "需要我帮你优化这段代码吗？",
    "options": ["是的", "不用了"]
  }
}
```

---

#### `aemeath_play` — 播放指定动画

强制切换到指定动画状态。

**参数：**
| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `state` | string | ✓ | 动画状态：`idle`/`thinking`/`running`/`review`/`failed`/`waving`/`jumping` |
| `duration_ms` | number | 可选 | 持续时间（毫秒），不指定则保持 |

**示例：**
```json
{
  "name": "aemeath_play",
  "arguments": {
    "state": "waving",
    "duration_ms": 3000
  }
}
```

---

#### `aemeath_get_user_input` — 获取用户输入（阻塞）

**阻塞等待**用户通过宠物 UI 输入，支持三种交互类型。

**参数：**
| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `prompt` | string | ✓ | 提示问题 |
| `type` | string | 可选 | 输入类型：`text`(默认)/`confirm`/`select` |
| `placeholder` | string | 可选 | 输入框占位文字（仅 text 类型） |
| `options` | string[] | 可选 | 选项列表（confirm/select 类型必需） |
| `timeout_secs` | number | 可选 | 超时秒数，默认 60，最大 300 |

**输入类型说明：**

| 类型 | UI 表现 | 返回值 |
|------|---------|--------|
| `text` | 输入框 + 提交按钮 | 用户输入的文本 |
| `confirm` | 是/否 两个按钮 | `"yes"` 或 `"no"` |
| `select` | 下拉选择框 | 选中的选项值 |

**示例 — 文本输入：**
```json
{
  "name": "aemeath_get_user_input",
  "arguments": {
    "prompt": "请输入文件名：",
    "type": "text",
    "placeholder": "example.rs",
    "timeout_secs": 120
  }
}
```

**示例 — 确认框：**
```json
{
  "name": "aemeath_get_user_input",
  "arguments": {
    "prompt": "确定要删除这个文件吗？",
    "type": "confirm",
    "timeout_secs": 30
  }
}
```

**示例 — 下拉选择：**
```json
{
  "name": "aemeath_get_user_input",
  "arguments": {
    "prompt": "选择构建模式：",
    "type": "select",
    "options": ["debug", "release", "test"],
    "timeout_secs": 60
  }
}
```

**返回值：**
- 成功：`{"content": [{"type": "text", "text": "用户输入值"}]}`
- 超时：`{"content": [{"type": "text", "text": "User did not respond (timeout)"}]}`
- 并发冲突：如果已有待处理的输入请求，返回错误 `code: -32603`

---

### MCP Resources

#### `aemeath://status`

获取当前宠物状态和动画信息。

**返回值示例：**
```json
{
  "contents": [{
    "uri": "aemeath://status",
    "text": "State: Running"
  }]
}
```

#### `aemeath://history`

获取最近的状态变更记录（最多 50 条）。

#### `aemeath://user-messages`

获取并清除用户通过宠物 UI 发送的消息。读取后消息即清除，空时返回 `(no pending messages)`。

---

## HTTP API (端口 9527)

### Hooks 端点（Claude Code 调用）

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/hook/thinking` | POST | 用户提交消息，宠物显示 "chatting" 动画 |
| `/api/hook/working` | POST | 工具开始执行，根据工具类型切换动画 |
| `/api/hook/done` | POST | 工具执行完毕，显示 "celebrating" 动画 |
| `/api/hook/idle` | POST | 会话空闲，切换为 idle 状态 |
| `/api/hook/permission` | POST | 权限请求，显示 "waving" 动画 + 提示 |

**Hook 状态映射：**

| Hook 端点 | 动画状态 | 气泡文字 |
|-----------|----------|----------|
| `thinking` | chatting | 正在组织回复... |
| `working` (Read/Glob/Grep) | running | 正在读取文件... |
| `working` (Write/Edit) | building | 正在构建... |
| `working` (Bash) | running | 正在执行命令... |
| `working` (WebFetch) | fetching | 正在获取网络内容... |
| `working` (WebSearch) | searching | 正在搜索网络... |
| `working` (Agent/Task) | analyzing | 正在分析... |
| `done` | celebrating | 太棒了! |
| `permission` | waving | 等待指示... |
| `idle` | idle | — |

### 查询端点（前端轮询）

#### GET `/api/current`

获取当前宠物状态。

**响应：**
```json
{
  "animation": "running",
  "bubble": "正在读取文件...",
  "core_signal": "running",
  "tool_label": "Read",
  "overlay": null
}
```

#### GET `/api/user/pending`

检查是否有待处理的用户输入请求。

**响应（有等待输入）：**
```json
{
  "waiting": true,
  "input_type": "text",
  "options": null
}
```

**响应（无等待）：**
```json
{
  "waiting": false
}
```

#### POST `/api/user/input`

前端提交用户输入（由宠物 UI 调用）。

**请求体：**
```json
{
  "value": "用户输入的内容",
  "type": "text"
}
```

#### POST `/api/user/message`

前端发送用户消息（用户主动通过宠物发送，非 MCP 请求）。
发消息时先通过 `EnumWindows` 实时搜索当前最前面标题含 "claude" 的窗口并粘贴。
搜不到时回退到启动时 `GetForegroundWindow()` 绑定的 HWND。

**请求体：**
```json
{
  "value": "用户通过宠物发送的消息"
}
```

#### GET `/api/user/message/pending`

获取并清除待处理的用户消息（供 MCP 资源使用）。

**响应：**
```json
{
  "count": 2,
  "messages": ["消息1", "消息2"]
}
```

### 管理端点

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/heartbeat` | GET | 健康检查，返回 200 OK |
| `/api/state` | POST | 直接设置状态（高级用法） |

---

## 双向交互流程

### 场景：Claude 需要用户输入

```
Claude Code                          Aemeath Pet
     │                                    │
     │  ┌─────────────────────────────┐   │
     │  │ 调用 aemeath_get_user_input │   │
     │  │ type: "select"              │   │
     │  └─────────────────────────────┘   │
     │ ─────────────────────────────────> │
     │                                    │
     │        JSON-RPC Request            │
     │                                    │
     │ <───────────────────────────────── │
     │        阻塞等待 oneshot channel    │
     │                                    │
     │                              ┌─────┴─────────┐
     │                              │ 广播 SSE 事件  │
     │                              │ overlay=input │
     │                              └─────┬─────────┘
     │                                    │
     │                              ┌─────┴─────────┐
     │                              │ 前端显示输入UI │
     │                              │ 下拉选择框    │
     │                              └─────┬─────────┘
     │                                    │
     │                              [用户选择选项]
     │                                    │
     │                              ┌─────┴─────────┐
     │                              │ POST /api/user/input
     │                              │ value="debug" │
     │                              └─────┬─────────┘
     │                                    │
     │                              ┌─────┴─────────┐
     │                              │ oneshot.send()│
     │                              │ 解除阻塞      │
     │                              └─────┬─────────┘
     │                                    │
     │ <───────────────────────────────── │
     │    {"content": [{"text": "debug"}]} │
     │                                    │
```

### 输入轮询机制

前端每 500ms 轮询 `/api/user/pending`：

1. 当 MCP 工具 `aemeath_get_user_input` 被调用时，后端创建 `PendingInput` 并阻塞
2. 前端轮询检测到 `waiting: true`，根据 `input_type` 显示对应 UI
3. 用户交互完成后，前端 POST 到 `/api/user/input`
4. 后端通过 oneshot channel 将值返回给 MCP 工具，解除阻塞

---

## 状态机

### 动画状态 (PetState)

| 状态 | 动画名 | core_signal | 气泡文案 |
|------|--------|-------------|----------|
| Idle | idle | idle | — |
| Thinking | waiting | waiting | — |
| Running | running | running | 工作中... |
| Review | review | ready | 搞定! |
| Failed | failed | idle | 好像出问题了... |
| Waving | waving | idle | 爱弥斯已上线~ |
| Jumping | jumping | idle | — |
| Chatting | chatting | running | 正在组织回复... |
| Fetching | fetching | running | 正在获取网络内容... |
| Searching | searching | running | 正在搜索网络... |
| Analyzing | analyzing | running | 正在分析... |
| Building | building | running | 正在构建... |
| Celebrating | celebrating | ready | 太棒了! |
| Permission | waving | waiting | 等待指示... |

### 核心信号层

用于前端简化状态判断：
- `idle` — 空闲、待机、可交互
- `waiting` — 等待用户输入或确认
- `running` — 正在工作中
- `ready` — 任务完成、成功状态

### 气泡锁机制

工具气泡有最短显示时间（800ms）：
- 当 Claude 快速连续执行多个工具时，不会频繁闪烁
- 只有非工作状态（celebrating/idle 等）且满足最短时间后才切换

---

## 配置参考

### Claude Code Hooks (`~/.claude/settings.json`)

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "",
        "hooks": [{
          "type": "command",
          "command": "powershell -Command \"if (-not (Get-Process -Name 'aemeath-claude' -ErrorAction SilentlyContinue)) { Start-Process 'D:/path/to/aemeath-claude.exe' }\""
        }]
      }
    ],
    "UserPromptSubmit": [
      {
        "matcher": "",
        "hooks": [{
          "type": "http",
          "url": "http://127.0.0.1:9527/api/hook/thinking"
        }]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "",
        "hooks": [{
          "type": "http",
          "url": "http://127.0.0.1:9527/api/hook/working"
        }]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "",
        "hooks": [{
          "type": "http",
          "url": "http://127.0.0.1:9527/api/hook/done"
        }]
      }
    ],
    "Stop": [
      {
        "matcher": "",
        "hooks": [{
          "type": "http",
          "url": "http://127.0.0.1:9527/api/hook/idle"
        }]
      }
    ],
    "PermissionRequest": [
      {
        "matcher": "",
        "hooks": [{
          "type": "http",
          "url": "http://127.0.0.1:9527/api/hook/permission"
        }]
      }
    ]
  }
}
```

### Claude Code MCP (`~/.claude/.mcp.json`)

```json
{
  "aemeath": {
    "type": "http",
    "url": "http://127.0.0.1:9528/mcp"
  }
}
```

---

## 端口速查

| 端口 | 协议 | 用途 |
|------|------|------|
| 9527 | HTTP | Hooks + 前端轮询 |
| 9528 | MCP (JSON-RPC) | 富交互 Tools/Resources |

# Aemeath v2 改造方案

> 目标：从"好看但信息量低的动画桌宠"升级为"直觉化 Agent 状态信号 + 互动增强"

---

## 核心理念

Codex Pets 的精髓不是"8 种形态"，而是 **3 个一眼看懂的状态**：

| 状态 | 你看到的 | 你该做的 |
|------|---------|---------|
| **running** | 爱弥斯在忙 | 别打扰，等她 |
| **waiting** | 爱弥斯停下来盯着你看 | 轮到你操作了 |
| **ready** | 爱弥斯开心/举东西 | 任务完成，去检查结果 |

现在的 15 个状态里，`Fetching`/`Searching`/`Analyzing`/`Building`/`Running`/`Chatting` 对用户来说**全是"在忙"**，区分它们靠的是气泡文字而不是行为直觉。这就是信息过载。

---

## 改造分 4 个阶段

### Phase 1：状态精简（改感最强，工作量最小）

**后端：15 状态 → 3 信号层 + 工具标签**

```
旧：PetState { Idle, Thinking, Running, Review, Failed, Waving, Jumping,
              Chatting, Fetching, Searching, Analyzing, Building, Celebrating, Permission }

新：
  CoreSignal { Running, Waiting, Ready }    ← 决定动画
  ToolLabel  { Read, Write, Edit, Bash, ... }  ← 决定气泡文字
  Overlay    { Permission, Error }          ← 特殊叠加层
```

`CoreSignal` 直接映射 spritesheet 动画：
- `Running` → 爱弥斯跑/忙碌（原 `running` / `running-right` / `running-left` 随机选）
- `Waiting` → 爱弥斯停下看着你（原 `waiting`）
- `Ready`   → 爱弥斯庆祝（原 `celebrating` / `review`）

`ToolLabel` 只控制气泡文字，不影响动画：
```
Read/Glob/Grep    → "正在读文件..."
Write/Edit        → "正在写代码..."
Bash              → "正在执行命令..."
WebFetch          → "正在获取网页..."
WebSearch         → "正在搜索..."
Agent/Task        → "正在调度子任务..."
其他              → "工作中..."
```

`Permission` 是叠加层：在任何 CoreSignal 上叠加 waving + "等待指示..." 闪烁。
`Error` 也是叠加：在 Ready 上叠加 failed 动画 + "出问题了..."。

**状态机转换规则（简化后）：**

```
UserPromptSubmit      → CoreSignal::Running
PreToolUse            → CoreSignal::Running  + ToolLabel
PostToolUse           → CoreSignal::Running  (保持，直到所有工具结束)
AllToolsDone          → CoreSignal::Ready
PermissionRequest     → Overlay::Permission  (叠加在当前信号上)
PermissionResolved    → 移除 Overlay，恢复原信号
Stop                  → CoreSignal::Idle     (回到空闲)
Error                 → Overlay::Error
```

**前端变化：**
- 气泡防闪烁逻辑保留（toolLockUntil 机制）
- 但不再按气泡内容判断 isToolBubble()，改为读 CoreSignal 字段
- permissionPending 逻辑保留

---

### Phase 2：事件驱动（去掉轮询）

**现状问题：**
- 前端 400ms 轮询 `GET /api/current`，状态变化最多延迟 0.4s
- Rust 端已经通过 `broadcast` channel 推送了 `StateChangeEvent`，前端没用上

**改造：**
```
前端：
  删除 pollState() 的 while(true) 轮询
  改用 Tauri event listener：
    window.__TAURI_INTERNALS__.listen('state-change', (event) => {
      const { animation, bubble, coreSignal } = event.payload;
      animator.play(animation);
      updateBubble(bubble, coreSignal);
    });

  保留 /api/current 轮询作为兜底（降频到 2s），防漏事件

后端：
  main.rs 已经有 broadcast → emit 转发，无需改动
  只需在 StateChangeEvent 加 coreSignal 字段
```

**收益：**
- 响应延迟从 400ms → <50ms
- CPU 占用降低（不再每 400ms 发 HTTP 请求）
- 前端代码更简洁

---

### Phase 3：气泡交互增强

**现状问题：**
- `aemeath_ask` 显示问题后立刻返回 `"User dismissed"`，用户无法回答
- 气泡只是纯文字，没有交互

**改造方案：双向气泡系统**

**3a. 按钮式回答（用于 aemeath_ask）**

```
前端新增：
  class InteractiveBubble extends Bubble {
    showWithOptions(question, options, callback) {
      // 显示问题 + 按钮列表
      // 用户点击按钮 → callback(answer)
      // 超时 60s → callback("timeout")
    }
  }

后端新增：
  POST /api/bubble/response  ← 前端把用户选择 POST 回来
  MCP aemeath_ask 改为：
    1. 发送气泡问题 + 选项给前端
    2. 轮询 /api/bubble/response（或用 SSE）
    3. 收到回答后返回给 Claude
```

**3b. 简单文字输入（可选）**

气泡上加一个小输入框，用户敲回车发送。用于 Claude 问"你想把变量命名为什么？"这类场景。

**3c. 实现路径**

```
前端：
  app.js 新增 _petAskState 存储当前问答
  bubble.js 加 showWithOptions() 方法
  index.html 加 #ask-bubble div（带按钮/输入框）

后端：
  mcp.rs aemeath_ask 改为 async 等待：
    - 设定 60s 超时
    - 用 oneshot channel 等前端 POST 回来
    - 超时返回 "User did not respond"
  http.rs 新增 POST /api/ask/respond 路由
```

---

### Phase 4：形态多样化（进阶，可选）

参考 Codex 的 8 种形态机制，但不照搬——你已经有了爱弥斯 IP。

**4a. 状态衍生形态（不需要新素材）**

利用现有 spritesheet 的 `running-left` / `running-right` 做方向感知：
- 窗口在屏幕左半边 → 爱弥斯朝右跑
- 窗口在屏幕右半边 → 爱弥斯朝左跑

加一个 idle 变体系统：
- 空闲 < 1min → 正常 idle
- 空闲 1-5min → 打瞌睡（idle 帧 + 眼睛半闭 CSS overlay）
- 空闲 > 5min → 睡着了（idle 帧 + Zzz 气泡）

**4b. 时间感知（不需要新素材）**

```
22:00-06:00 → 爱弥斯困倦态（动画速度减半 + 困倦气泡）
周末        → 爱弥斯休闲态（idle 变体概率提高）
```

**4c. 自定义形态孵化（需要新素材，工作量大）**

参考 Codex 的 `/hatch`：
- 用 Seedream 生成新形态的 spritesheet
- 用户上传图片 → AI 生成像素风桌宠
- 注册到前端 frameMap

这个优先级最低，因为需要大量素材工作。

---

## 实施顺序和工作量

| 阶段 | 改动文件 | 工作量 | 体验提升 |
|------|---------|--------|---------|
| Phase 1 | state.rs, http.rs, mcp.rs, app.js | 2-3h | ★★★★★ 最核心 |
| Phase 2 | app.js, state.rs (加字段) | 1h | ★★★ 响应更快 |
| Phase 3 | mcp.rs, http.rs, app.js, bubble.js, index.html | 3-4h | ★★★★ 交互质变 |
| Phase 4a | app.js, css | 1h | ★★ 锦上添花 |
| Phase 4b | app.js | 30min | ★ 微妙但有趣 |
| Phase 4c | 全栈 + 素材 | 8h+ | ★★★ 长期价值 |

---

## 技术细节备忘

### StateChangeEvent 改造
```rust
// 旧
pub struct StateChangeEvent {
    pub animation: String,
    pub bubble: String,
}

// 新
pub struct StateChangeEvent {
    pub animation: String,
    pub bubble: String,
    pub core_signal: String,  // "running" | "waiting" | "ready" | "idle"
    pub tool_label: Option<String>,  // "Read" | "Bash" | ...
    pub overlay: Option<String>,     // "permission" | "error" | None
}
```

### hooks.json 无需改动
Claude Code 的 hook 事件（UserPromptSubmit/PreToolUse/PostToolUse/Stop/PermissionRequest）映射关系不变，只是后端内部状态机从 15 状态简化为 3+ToolLabel。

### 向后兼容
MCP tool `aemeath_play` 的参数枚举需要更新：
```json
// 旧
"enum": ["idle", "thinking", "running", "review", "failed", "waving", "jumping"]

// 新
"enum": ["idle", "running", "waiting", "ready", "waving", "jumping"]
```

---

## 不做的事（明确排除）

- ❌ 不加成长/喂食/好感度系统（不是 Tamagotchi）
- ❌ 不加音效（桌面环境不适合）
- ❌ 不加网络排行榜/社交功能
- ❌ 不做跨平台（只 Windows，Tauri 限制）
- ❌ 不做自定义皮肤系统（Phase 4c 用 AI 生成，不做手动编辑器）

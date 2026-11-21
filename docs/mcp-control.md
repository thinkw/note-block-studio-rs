# 用 MCP 让 AI 操控正在跑的 nbs-rs

这一页只讲接线和口径，不讲实现。目标是一件事：**编辑器窗口开着 + IDE 里填好一条 MCP 配置，AI 就能动这个窗口里的工程**。

## 1. 准备两个可执行文件

```bash
cargo build --release -p nbs-app -p nbs-mcp
```

出来的是 `target/release/nbs-rust.exe`（编辑器）和 `target/release/nbs-mcp.exe`（桥接）。
桥接是个薄壳：它在 MCP 那一侧讲 stdio，在编辑器那一侧连本机回环端口，自己不存任何工程数据。
IDE 配置里要填的是**后者的绝对路径**，机器上只留一份，别在多处复制。

## 2. IDE 里填 MCP 配置

```json
{
  "mcpServers": {
    "nbs-rs": {
      "command": "D:/develop/RustProject/nbs-rs-mcp/target/release/nbs-mcp.exe"
    }
  }
}
```

不需要 `args`：桥接默认去编辑器的配置目录读发现文件（见第 4 节）。
`--client-name` 可选，填了会出现在编辑器「外部控制」窗口的连接名单里，方便认出是哪客户端连的。

## 3. 编辑器那一侧要先答应（默认不答应）

外部控制**默认关**。三种开法，优先级是命令行 > 环境变量 > 设置页：

| 开法 | 怎么做 | 能开到什么程度 |
| --- | --- | --- |
| 设置页 | 窗口里勾「启用外部控制（打开窗口就监听）」 | 监听 +（单独一条勾）危险动作 + 读写目录 |
| 环境变量 | `NBS_RUST_CONTROL=1` | 只有监听。它能开通路，**永远**给不出危险动作 |
| 命令行 | `nbs-rust.exe 歌.nbs --control` | 监听；`--control=端口` 指定端口；`--control-dir=<目录>` 可重复地加读写目录；`--control-danger` 放危险动作 |

想临时按住不改设置页，用 `NBS_RUST_CONTROL=0` 起一次就行。
关掉要在窗口里取消勾选——只把 IDE 配置删掉不算关，端口还在听。

危险动作是四类：退出程序、用浏览器打开链接、替你回答界面确认框、清空「最近打开」。
默认全挡，界面会说「危险动作仍被挡」。没想清楚就别勾——那一条勾等于允许 AI 替你按「退出」。

## 4. 桥接怎么找到窗口

编辑器答应之后，会在配置目录写一份 `control.json`：端口、令牌、协议版本、pid、应用名。
桥接读它，然后连 `127.0.0.1:<端口>`。目录规则两边共用同一份实现：

1. `NBS_RUST_CONFIG_DIR` 指哪儿算哪儿；
2. Windows：`%APPDATA%\nbs-rust`；
3. Linux：`$XDG_CONFIG_HOME/nbs-rust`，再退 `$HOME/.config/nbs-rust`；
4. 都没认出来时兜底 `./.nbs-rust`。

**能不能连上，取决于谁能读这份文件里的令牌**——同机能读到 `control.json` 的进程就能连。
这就是全部的访问控制，服务只绑 `127.0.0.1`，不监听外网网卡，并发连接上限 8。
别把配置目录放进共享盘或同步盘。

连不上只有三种原因，报错会说是哪一种：编辑器没起、编辑器没答应（没写发现文件）、目录算错。
第三种用 `--config-dir <目录>` 顶掉，或者干脆 `--port <端口> --token <令牌>` 直连（令牌就在这份文件里）。

## 5. 不开 IDE 也能问一句：一次性命令行

桥接除了当 stdio 服务，还能一条一条直接调，脚本不用写握手：

```bash
nbs-mcp --call get_snapshot
nbs-mcp --call get_notes '{"tick_from":0,"tick_to":31}' --json
nbs-mcp --call clear_song '{"discard_changes":true}'
nbs-mcp --call set_notes '{"cells":[{"tick":0,"layer":0,"instrument":0,"key":"C4"}]}'
```

默认打一行状态：

```
set_notes ok rev=6 cells_changed=1 layers_changed=0 meta_changed=false dirty=true data=file "…"
```

要看整份回执 JSON 就加 `--json`。退出码分得开，是因为该改的东西不一样：

- `0` 编辑器答成；
- `1` 编辑器答不（闸门、参数、当前状态），原因在 `error.code`；
- `2` 没连上或命令本身不对——该动的是启动方式或参数拼法，不是工程。

核对「AI 会看到什么口径」不需要编辑器在场：

```bash
nbs-mcp --names                        # 38 条工具名
nbs-mcp --dump-tools set_notes         # 只看一条：说明 + 入参 schema
nbs-mcp --dump-tools set_notes --json  # 同一条的机器可读版
```

`--dump-tools <工具>` 就是失败回执里那句「形状看 `--dump-tools …`」指的写法；名字打错会直接说「工具面里没有这条」，不会打一份空表装作成功。

## 6. 结果怎么读

MCP 那一侧的 `isError`：**两层里任何一层答「不」，`isError` 都是 true**——桥接没碰到编辑器（`transport` 有值），或编辑器答了不（`outcome.ok == false`）。
失败时也把整份回执原样放进 `structuredContent`，所以读完 `error.code` 再决定下一步，别接着往下写。

`error.code` 是对外契约，字符串不会随便换：
`bad_argument` / `not_found` / `locked_layer` / `rev_conflict` / `modal_open` / `window_open` / `busy` / `unsupported` / `not_allowed` / `io` / `out_of_range` / `timeout` / `closed` / `internal`。

每条写操作的回执都带 `report`：`rev`、`cells_changed`、`layers_changed`、`meta_changed`、`dirty`、`history_len`。
`cells_changed` 可能小于你给的条数——写下去和原来一样的格子不算改动。

## 7. 闸门：不带就等于不认错

这几个开关默认全是「不」，因为「AI 猜你想要覆盖」和「你想要覆盖」是两件事：

- `overwrite`（`save_as` / `export`）：目标是别人的文件时必须为 true；
- `discard_changes`（`clear_song` / `open_file` / `import_midi` / `exit`）：有未保存改动时必须为 true，这些动作连撤销历史一起丢；
- `expect_rev`（所有写操作）：填上一次回执里的 `rev`，对不上就 `rev_conflict` 而不是覆盖掉别人刚改的。省略 = 不校验，直接覆盖当前状态；
- 读写目录：路径必须落在编辑器放行的目录里（设置页那栏 + `--control-dir`）；
- 有模态框挂着时写操作回 `modal_open`（只读操作不受影响）。

## 8. 写音符的口径

- `tick` / `layer` 都从 0 起算，和 `.nbs` 文件一致；
- `key` 是 0..=87 的键号，**也收音名**：`45` 与 `"F#4"` 等价，`"C4"`、`"Bb4"`、`"F♯3"` 都认。等音异名（`"Cb4"`、`"B#3"`）和不写八度（`"C"`）会被拒而不是被猜——它们在键盘上没有唯一位置；
- Minecraft 原生只播 33..=57。`get_instruments` 的回执里带 `mc_key_range`，就是编辑器判 `out_of_range` 用的同一对数；超出范围的音符会照写，但会标出来，导出后是静音音符盒；
- `instrument` 先问 `get_instruments` 再写：内置 0..=19，自定义从 20 起，随手写个不存在的号会得到 `bad_argument`；
- `velocity` 省略取 100，`panning` 省略取 100（中央），`pitch`（音分）省略取 0；
- 一次 `set_notes` = 一次撤销，整批先校验后落笔，要么全成要么全不动。

## 9. 这套面做不到的事

- **没有截屏**，也没有任何「看一眼界面」的能力。想知道现状就调 `get_snapshot` / `get_notes` / `get_diagnostics`，它们是这件事的替代；
- 必须靠鼠标拖动的流程没有等价物，会回 `unsupported`；能列出有哪些动作的，是 `list_actions` + `run_action` 那对逃生舱；
- schematic / datapack 导出没接进工具面（`export` 只到 wav / midi / png）；
- 撤销栈有容量上限，`get_history` 会告诉你游标和长度；
- 它操作的是**正在跑的那个窗口**，不是磁盘上的文件：想让 AI 改完再存，要显式 `save`。

## 10. 排查顺序

1. `nbs-mcp --call ping` —— 一句就知道通不通；
2. 不通先确认编辑器答应了吗：设置页那栏会写「正在监听 127.0.0.1:<端口>」和发现文件全路径；
3. 路径对不上就 `--config-dir`，或者从 `control.json` 里取端口和令牌走 `--port` / `--token`；
4. 连上了但一条都调不动：看 `error.code`，`modal_open` / `window_open` 是界面挡着，`not_allowed` 是闸门没开；
5. IDE 侧看不到工具：确认 `command` 指向的是 `nbs-mcp`，不是 `nbs-rust`。

日志和给人说的话一律走 stderr，stdout 只放协议字节和脚本要的产物——所以「IDE 里配置没生效，直接跑一下看看」这件事在 `--call` 上是能做的。

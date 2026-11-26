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

参数几百个格子时别塞进命令行——shell 引号比调用本身更容易出错，用 `--call-file` 换来源，路径给 `-` 读标准输入：

```bash
nbs-mcp --call set_notes --call-file cells.json
nbs-mcp --call set_notes --call-file - < cells.json
```

文件里是空的（或只有空白）等于没给参数；读不到文件、JSON 坏掉都在连编辑器之前退出（码 2），报错会点名是哪一份来源。

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
nbs-mcp --names                        # 46 条工具名
nbs-mcp --dump-tools set_notes         # 只看一条：说明 + 入参 schema
nbs-mcp --dump-tools set_notes --json  # 同一条的机器可读版
```

`--dump-tools <工具>` 就是失败回执里那句「形状看 `--dump-tools …`」指的写法；名字打错会直接说「工具面里没有这条」，不会打一份空表装作成功。

## 6. 结果怎么读

MCP 那一侧的 `isError`：**两层里任何一层答「不」，`isError` 都是 true**——桥接没碰到编辑器（`transport` 有值），或编辑器答了不（`outcome.ok == false`）。
失败时也把整份回执原样放进 `structuredContent`，所以读完 `error.code` 再决定下一步，别接着往下写。

`error.code` 是对外契约，字符串不会随便换：
`bad_argument` / `not_found` / `locked_layer` / `rev_conflict` / `tab_gone` / `parked_tab_readonly` / `requires_active_tab` / `expect_required` / `needs_confirm` / `modal_open` / `window_open` / `busy` / `unsupported` / `not_allowed` / `io` / `out_of_range` / `timeout` / `closed` / `internal`。

每条写操作的回执都带 `report`：`rev`、`cells_changed`、`layers_changed`、`meta_changed`、`dirty`、`history_len`。
`cells_changed` 可能小于你给的条数——写下去和原来一样的格子不算改动。
`open_file` / `import_midi` 是整首替换，撤销历史一起清空，算不出差量，所以这两条的 `cells_changed` / `layers_changed` 是**新工程的规模**（几个音符、几层）；`song_session` 同时加一，用它判断「工程被换过」比看数字更直接。

## 7. 闸门：不带就等于不认错

这几个开关默认全是「不」，因为「AI 猜你想要覆盖」和「你想要覆盖」是两件事：

- `overwrite`（`save_as` / `export`）：目标是别人的文件时必须为 true；
- `discard_changes`（`clear_song` / `open_file` / `import_midi` / `exit`）：有未保存改动时必须为 true，这些动作连撤销历史一起丢；
- `expect_rev`（写操作）：填上一次回执里的 `rev`，对不上就 `rev_conflict` 而不是覆盖掉别人刚改的。当前标签省略 = 不校验；后台标签写必填，缺了回 `expect_required`；
- 读写目录：路径必须落在编辑器放行的目录里（设置页那栏 + `--control-dir`）；
- 有模态框挂着时写操作回 `modal_open`（只读操作不受影响）。

### 标签寻址（协议 v2）

- 请求信封新增可选 `tab`。`{"id":稳定会话号}` 或 `{"index":标签条位置}` 二选一；两个都给或都不给回 `bad_argument`。省略 `tab` 的旧调用仍打当前标签。
- `id` 只在标签关闭时失效，重排不影响；`index` 从 0 起算，重排后会指向别的标签。长期持有引用用 `id`，关闭后的引用回 `tab_gone`，不要当 `rev_conflict` 重试。
- `list_tabs` 返回每首的 `id/index/title/filename/dirty/rev/total_blocks/song_length/layer_count` 与 `active_tab`；`switch_tab` 切标签并停播；`close_tab` 关闭指定标签。
- `get_snapshot/get_notes/get_layers/get_selection/get_history/get_instruments` 接受 `tab`。读后台标签不切歌、不重算音频或特效；后台快照的 `playhead/playing/modal_count` 为 `null`，`active_tab` 与 `tabs` 仍描述真实标签条。
- 后台**文档级写默认关闭**。设置页「允许修改后台标签」开启后，`set_notes/remove_cells/batch_field/transpose/select/place_selection/layer_*/set_meta/undo/redo` 可带 `tab + expect_rev` 执行；历史与撤销仍走各标签自己的 `Document`，不动当前标签的音频/特效缓存。
- 播放、seek、文件、导入导出、任务和 `run_action` 等全局/交互操作打后台标签回 `requires_active_tab`，任何档位都不放开。
- `close_tab` 遇到脏标签且没带 `discard:true` 时回 `needs_confirm`，不会挂一个无人回答的弹窗；明确丢弃才关闭。
- `run_action("answer_modal", {"answer":"yes"|"no"|"cancel"})` 支持三态；旧的 `{"yes":true/false}` 仍兼容。它仍受危险动作开关约束。

## 8. 写音符的口径

- `tick` / `layer` 都从 0 起算，和 `.nbs` 文件一致；
- `key` 是 0..=87 的键号，**也收音名**：`45` 与 `"F#4"` 等价，`"C4"`、`"Bb4"`、`"F♯3"` 都认。等音异名（`"Cb4"`、`"B#3"`）和不写八度（`"C"`）会被拒而不是被猜——它们在键盘上没有唯一位置；
- Minecraft 原生只播 33..=57。`get_instruments` 的回执里带 `mc_key_range`，就是编辑器判 `out_of_range` 用的同一对数；超出范围的音符会照写，但会标出来，导出后是静音音符盒；
- `instrument` 先问 `get_instruments` 再写：内置 0..=19，自定义从 20 起，随手写个不存在的号会得到 `bad_argument`；
- `velocity` 省略取 100，`panning` 省略取 100（中央），`pitch`（音分）省略取 0；
- 一次 `set_notes` = 一次撤销，整批先校验后落笔，要么全成要么全不动。

音名↔键号那张表是**从 A 起**的（`A0=0`、`B0=1`、`C1=2`…，八度号跟着 NBS 惯例走），别按「C0=0」心算：
按 C 起算会把 sol/la/si 那一档整体低半音，而写进去和读回来用的是同一张表，所以两头都对得上、只有耳朵能发现。
要确认某张表就 `get_instruments`（带 `mc_key_range`）或直接问一句 `nbs-mcp --dump-tools set_notes`。

## 9. 播放与网格的口径

- **一格（tick）多长** = `1 / tempo` 秒：`tempo` 是「每秒走几格」，tempo 10 时 1 秒走 10 格，不是 1 秒 1 格；
- **`tempo` 不是 BPM**：BPM = `tempo × 15`。`get_snapshot` 两个字段都给（`tempo` 和 `bpm`，后者就是界面那行的数），
  想按 BPM 定速度就 `set_meta {"tempo": BPM / 15}`，写完回读 `bpm` 确认；
- **小节**：每节 `time_signature × 4` 格（默认拍号 4 → 16 格一小节），这就是界面竖线的间距。
  拍号在读写时夹在 2..=8，所以 12/8 那类文件进来会落到 8 → 32 格一节；
  按小节读旋律就 `nbs-mcp --call get_notes '{"bars":[0,3]}' --json`，
  回执的 `data.bar_overview` 每节一行 `tick:音名(音色)`（音名由状态栏同一张表生成，不用自己把键号心算回去）；
  想换宽度加 `ticks_per_bar`（可与 `bars` 同给），一次最多 256 节，`bars` 与 `tick_from` / `tick_to` 互斥；
- **`play` 的落点**：从当前播放头起播；播放头已经到或越过曲尾时先归零，回执说一句「已归零」。
  需要显式起点就先 `seek` 再 `play`；
- `pause` 留住播放头，`stop` 把播放头送回 0，`seek` 的 tick 可以是小数（落在两格之间）；
- `set_loop` 只改循环开关，别的元数据不动。

## 10. 导入 MIDI：先预演，再落歌

`inspect_midi` 和 `import_midi` 吃同一套参数，区别只在要不要换掉当前工程：

```bash
nbs-mcp --call inspect_midi '{"path":"D:/songs/jazz.mid"}' --json
nbs-mcp --call import_midi '{"path":"D:/songs/jazz.mid","discard_changes":true}'
```

预检跑的是真的导入算法，算完就丢，`rev` 一点不动。它回的核对单和导入回执同一个形状：音符数、层数、层名、`length_ticks`、tempo 与 BPM、超 MC 音域的个数、按映射判成静音的个数，逐通道 / 逐鼓音 / 逐声部的计划，加上这次**真正生效**的选项全表。先预演再落歌，是给「我以为这份文件长这样」找一个可以对的地方。

- 省略选项 = 复刻人坐在导入窗前看到的那份，不是出厂默认值：通道与鼓映射按 GM 表现算，标量取自偏好。出厂那份的鼓表是空的，照它导会把打击乐整段判成静音。
- 选项里的字段名**拼错会被当场拒**（这几份结构体不认未知字段）。「写错」和「省略」在结果上是两件事：省略该走基线，写错不该悄悄变成基线。
- `channels` 不含通道 9，9 的映射写在 `drums`，`note` 要用预检给你的那些音号；`instrument: -1` 是「这条通道不要」，不是「用 0 号音色」。
- `enhanced` 是窗上那套延音 / 拆声部 / 去重 / 越界配器。只给子字段却不给 `enabled: true` 会被拒——你以为在调延音，实际走的是传统路径，那才是最难查的一种没生效。越界替换表 `substitutes` 只覆盖内置 0..=19，给一行就整行替换出厂那一行。
- `remember: true` 是这条链上唯一会动用户长期设置的开关：把这次的映射写回 GM 表并存进偏好，等于替人点了窗上那个「记住」。省略就是 false，想改一次而不留痕就别给。
- `precision` 收 0 / 1 / 3 / 7，对应窗上 1 / 2 / 4 / 8 倍；它同时决定 tick 密度，同一份文件 8 倍比 1 倍长四倍。

## 11. 音频转 MIDI：许可、环境，和一条必须点名的交付路径

四条件对应转录窗上的四个按钮，全部**立即回任务句柄**（python 子进程动辄几十秒到几十分钟，
不让一次调用憋着，也不让一条请求挂住界面线程），之后 `job_status` 轮询、`job_cancel` 能停：

| 工具 | 要许可 | 要装好的环境 | 干什么 |
| --- | --- | --- | --- |
| `transcribe_probe` | 否 | 否 | 问 Python / torch / MuScriptor 版本和 GPU 情况，不下载任何东西 |
| `transcribe_prepare_env` | 是 | 否 | uv 建 venv、装 MuScriptor 与 torch（几百 MB） |
| `transcribe_prepare_model` | 是 | 是 | 只取权重并加载一遍，先验显存与精度，不识别音频 |
| `transcribe_audio` | 是 | 是 | 音频 → MIDI，`result` 带产物与核对数 |

**许可那一下只能人自己勾。** MuScriptor 权重是 CC BY-NC 4.0、环境要现装几百 MB，所以
「确认本地下载和模型使用许可」这个勾只认人在「音频转 MIDI」窗口里点的那一下：
AI 面没有代勾的入口，`options` 里也没有这个字段，`list_actions` 的逃生舱里同样没有它。
没勾时三条得到的是 `not_allowed`，那句话会点名该谁去勾——**这不是接线故障，别绕**。

`options` 省略哪个字段就用窗口里当前那一份（和 MIDI 导入同一个口径），字段名拼错当场拒。
取值域在这儿就钉死了：`model` 是 `small`/`medium`/`large`，`device` 是 `auto`/`cpu`/`cuda`，
`dtype` 是 `auto`/`float32`/`float16`，`beam_size` 是 `1..=4` 的整数，`temperature` `0..=2`，
`cfg_coef` `0..=3`，`model_path` 要本地 `.safetensors`（给空串是显式清掉窗上那行）。
错在起子进程之前，是为了三十秒后回来的不是「任务失败」而是一句能照着改的话。

**要接着导入就必须给 `save_midi`。** 转录产物本来落在编辑器自己的
`<workspace>/.work/transcription/jobs/…`，那个目录**不在**路径白名单里，
`import_midi` 读不到它——这是刻意的：白名单是这台机器上唯一的门禁，不该因为
「文件是我自己生成的」就放行。给一个落在放行目录里的 `.mid` 路径，
覆盖别人的文件照样要 `overwrite=true`，之后接现成的那条链：

```bash
nbs-mcp --call transcribe_probe
nbs-mcp --call job_status '{"id":1}'
# 人在窗口里勾过许可之后：
nbs-mcp --call transcribe_prepare_env '{"cuda":false}'
nbs-mcp --call transcribe_audio '{"audio":"D:/in/take.wav","save_midi":"D:/out/take.mid","overwrite":true}'
nbs-mcp --call job_status '{"id":3}' --json
nbs-mcp --call inspect_midi '{"path":"D:/out/take.mid"}' --json
nbs-mcp --call import_midi '{"path":"D:/out/take.mid","discard_changes":true}'
```

转录任务的 `result` 除了 `raw_note_count`（模型报的原始音符数），还带回窗上那两条核对：
`preview_notes`（预览读到的音符）、`importer_events`（NBS 导入器读到的事件数）、
`note_count_mismatch`（两者不一致）。对不上时**窗口里那个「导入」按钮会直接停止**，
这里照实报而不替人决定——真正会落进歌里的数量看下一步 `inspect_midi`，
它按你准备用的那套选项算，且 `import_midi` 会拒掉算不平的结果。

一次只跑一个转录任务：这四条共用同一份 venv 和权重缓存，并发跑出来的失败形状很难看。
窗口里已经有一个在跑、或任务表里已经排着一个，都会得到 `busy`。

## 12. 这套面做不到的事

- **没有截屏**，也没有任何「看一眼界面」的能力。想知道现状就调 `get_snapshot` / `get_notes` / `get_diagnostics`，它们是这件事的替代；
- 必须靠鼠标拖动的流程没有等价物，会回 `unsupported`；能列出有哪些动作的，是 `list_actions` + `run_action` 那对逃生舱；
- schematic / datapack 导出没接进工具面（`export` 只到 wav / midi / png）；
- 转录窗上的 SoundFont A/B 试听没接进来：要听识别出来的效果，用 `export`（wav 是离线渲染，不需要那个几百 MB 的音色库勾）；
- 撤销栈有容量上限，`get_history` 会告诉你游标和长度；
- 它操作的是**正在跑的那个窗口**，不是磁盘上的文件：想让 AI 改完再存，要显式 `save`。

## 13. 连接会闲下来：断了之后桥接怎么处理

事实先摆出来（`nbs-control/src/server.rs` 与 `client.rs`，不是推测）：

- 服务端每条连接的读超时是 `request_timeout × 2`，默认 **60s**；空闲满 60s，那条连接的线程就自己退出了，服务端也不登记在册了。
- 桥接等一条回执的上限是 **45s**（故意小于服务端的 60s 总闸，让主线程先说完「我排到了吗」）。
- `--call` 每次调用都新建一条连接，所以它**看不见**这个现象——只有常驻的 MCP 会话（IDE 里那条 stdio）会长时间空闲。

症状就一句话：**开局几条全成，放着不动一阵子之后每条都失败，而且重启 IDE 之前不会自己好。**

现在桥接的处理口径（`nbs-mcp/src/lib.rs` 的 `dispatch_link`）：

1. 任何传输层失败——连不上、对端断了、等回执超时——**一律作废这条连接**，连同 socket 一起丢掉。
   不能「放弃这一答、接着用这条」：我们不等了，服务端还在往这条连接上写那一答。
   留着它，下一问先读到的是上一问那张过期回执（旧 `rev`、旧计数），看着像答对了其实答的是过去的事；
   更坏的一版是那一答压根没来，于是每问都慢到等满 45s。
2. 下一问从新握手开始重连。
3. **只有只读工具会自动重发一次**。只读 = `Op::is_read_only()` 那 13 条，对应工具名：
   `ping`、`list_actions`、`get_snapshot`、`get_notes`、`get_layers`、`get_selection`、
   `get_instruments`、`get_diagnostics`、`get_history`、`list_tabs`、`job_status`、`job_list`、`inspect_midi`。
   重发之后拿到的是新连接上现算的答案，`rev` 也是那时的；桥接会往 stderr 写一行「断过一次」，日志里翻得到。
4. **写操作绝不自动重发**：请求可能已经落进主线程的收件箱了，重发一遍就是改两回。
   这种失败回的是 `transport`，文案会点名「没有替你重发」，并让你先用 `get_snapshot` 对一眼 `rev` 和变更量再决定要不要重放。
   连接照样重建好了，下一问站在新的那条上。

`server_info` 现在当场敲一次 `ping` 再答，分两格：`bridge` 那格是桥接自己（不用编辑器在场也答得出），
`editor` 那格是问出来的「现在接不接得住」，`parked_write` 是最近一次握手公布的标签能力。接不住时整条走 `transport`，所以 MCP 侧 `isError=true`、
`--call server_info` 的退出码是 **2**（没连上）而不是 0——「面板 ready」不等于「编辑器接得住」。

另外：IDE 比编辑器先启动也不会把服务顶死了。旧做法开局连不上就 `exit(2)`，客户端只看见「MCP 服务起不来」，
窗口后来开好了也没用，得重启 IDE；现在服务照样起来、工具面照样全在，第一问再连。

**故意没做的两件事**：把 60s 空闲超时取消，以及加一条保活心跳（每 30s 发一个空行）。
心跳能续命，但服务端的取消闸只在解析出一条真请求之后才检查，而 `stop()` 手里没有连接登记表——
只发心跳的客户端会在人取消勾选「启用外部控制」之后继续占住一条线程和一个客户端名额（上限 8）。
这和「关掉必须是立刻关掉」那条口径冲突，要做得连着登记表和闸一起做，不是一个空行能收口的。
长导出这类重活本来就走 `job_status` 轮询，不会把一条连接按在主线程上 45s 以上，所以这里不需要为它放宽上限。

## 14. 排查顺序

1. `nbs-mcp --call ping` —— 一句就知道通不通；想知道「桥接活着但编辑器接不住」是哪一格，读 `server_info` 的 `editor`；
2. 不通先确认编辑器答应了吗：设置页那栏会写「正在监听 127.0.0.1:<端口>」和发现文件全路径；
3. 路径对不上就 `--config-dir`，或者从 `control.json` 里取端口和令牌走 `--port` / `--token`；
4. 连上了但一条都调不动：看 `error.code`，`modal_open` / `window_open` 是界面挡着，`not_allowed` 是闸门没开；
5. 失败的是 `transport` 而不是 `error.code`：那是桥接这一层没送到，跟参数无关，别去改 JSON。
   「放着不动一段时间之后全失败」就是这一类，见 §13——读操作桥接自己会重连重发，写操作要你先对一眼 `rev`；
6. IDE 侧看不到工具：确认 `command` 指向的是 `nbs-mcp`，不是 `nbs-rust`。

日志和给人说的话一律走 stderr，stdout 只放协议字节和脚本要的产物——所以「IDE 里配置没生效，直接跑一下看看」这件事在 `--call` 上是能做的。

## 15. 编译时 `failed to remove file nbs-mcp.exe`

症状：`cargo build --release` 到最后一个 crate 报

```
error: failed to remove file `D:\develop\...\target\release\nbs-mcp.exe`
Caused by: 拒绝访问。 (os error 5)
```

而且同一次构建里 `nbs-rust.exe` 已经换掉了——只有这一个删不动。

**这不是调用失败，是文件占用**：Windows 上正在跑的 exe 镜像不让删也不让覆盖，cargo 只能先删后写。
持锁的是那条**还在跑的 `nbs-mcp.exe` 进程**，它的父进程是 IDE 的会话 worker，不是 nbs-rs 编辑器。

两件事因此管不着它：

- 编辑器里取消勾选「启用外部控制」——那停的是**编辑器进程内**的 TCP 服务端，桥接是另一个进程；
- 关掉编辑器窗口——桥接退出只看 stdin 有没有被客户端关掉（`serve` 在等 `waiting()`），编辑器在不在它不管。

IDE 里把这条 MCP 服务关掉通常会杀掉进程；但**配置开关不保证回收已经派生出来的那一条**，
所以残留的桥接会一直占着，直到那个会话或 worker 退出。

先认出是哪条（它的命令行就是被锁的那份路径）：

```
tasklist | findstr /i nbs-mcp
wmic process where "name='nbs-mcp.exe'" get ProcessId,ParentProcessId,CreationDate,CommandLine /format:list
```

按打扰从小到大三条路：

1. `taskkill /PID <那条> /F`。桥接不存任何状态，回执都是当场问编辑器要的；IDE 里重新启用会拉一条新的。
2. 把 IDE 的 MCP `command` 指到 `target` 目录**之外**的一份副本（发布后拷过去），
   以后 cargo 怎么写 `target/release` 都不会和它抢同一个镜像。
3. 开发期换输出目录：`cargo build --release --target-dir target-dev`。

这是 §13 那轮口径带来的代价，不是缺陷：那一轮特意让「开局连不上也照样把服务起来」，
就是为了不再逼你重启 IDE；常驻就意味着一直占着自己那份 exe。
以前 IDE 比编辑器先启动时它会 `exit(2)`，反倒不占锁——同一件事换了两面。

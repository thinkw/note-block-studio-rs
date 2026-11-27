# Third-party notices（第三方素材与代码声明）

本仓库的源代码与文档按 `LICENSE` 里的 **AGPL-3.0-only** 分发。AGPL 只处分我们自己拥有
著作权的那部分：下面列出来的东西**不在 AGPL 授权范围内**，它们的权利人和条款各自保留。
这一页的作用是——任何人从本仓库拿东西时，能一眼看出哪些不是我们的、以及我们凭什么分发
它们。目录级的短声明另见 `assets/Blocks/MOJANG-NOTICE.txt` 与
`assets/icons/SOURCE.txt`。

数字都是当场 `git ls-files` + 文件字节数统计的（截至 2026-11-22，提交 `da197da` 之后）。

## 一、Mojang / Microsoft 的像素与音频

这些是 Minecraft 原作内容的复制或衍生，所有权在 Mojang Studios / Microsoft，我们只是
按他们的「粉丝与创作者使用指引」分发一小部分。

| 路径 | 数量 / 体积 | 是什么 | 来路 |
|---|---|---|---|
| `assets/Blocks/*.png` | 20 张 / 4,147 B | 20 个内置乐器在播放特效里画到的方块**面**贴图 | Mojang 客户端 jar 内 `assets/minecraft/textures/block/`，版本与 sha1 记在 `assets/Blocks/PROVENANCE.json`，抓取工具 `tools/fetch_block_textures.py`。同目录抓出来的其余 1019 张被 `.gitignore` 忽略，不入库 |
| `assets/BlocksIso/*.png` | 20 张 / 96,478 B | 同一批方块的**等距机体渲染图**（300×300，播放特效里只有声道方块的摞塔画这一层；方块排和瀑布的「乐器方块」用上面那批 16×16 面贴图） | Minecraft Wiki（zh.minecraft.wiki）各方块条目信息框的 `File:` 图。Wiki 前面有 Anubis 人机验证，脚本扒不到，是浏览器人手另存后按 `resolve_file` 的主名改名的；原名、字节数、sha256 记在 `assets/BlocksIso/PROVENANCE.json`，声明在 `assets/BlocksIso/WIKI-NOTICE.txt` |
| `assets/Sounds/*.ogg` | 20 段 / 128,512 B | note block 各乐器的原版采样（harp / bdrum / trumpet…） | 上游 OpenNBS 装机版 `Data\Sounds` 原样拷贝，我们没有重录或 remix |
| `assets/icons/{block,block_color,block_color_shape,block_sel,block_shape,instrumenticons}/` | 145 帧 | 音符盒方块精灵、按方块上色的音符帧、20 个内置乐器的 16×16 方块图标 | 从上游 GameMaker 工程导出（`tools/export_sprites.py`），内容本身是 Minecraft 素材的衍生 |
| `assets/extranotes.zip` | 1 个 / 462,095 B | 额外音符贴图包，装机版原样拷贝（md5 记在 `assets/icons/SOURCE.txt` 末段） | 同装机版；我们 `include_bytes!` 内嵌的是同一份字节 |

等距图那 20 张是**两层权利叠在一起**：底层像素还是 Mojang 的（第二节那几句原文同样管着
它们），Wiki 站方对上传文件通常标 **CC BY-NC-SA 4.0**（署名 + 非商业 + 相同方式共享）。
这 20 张的许可栏我们没逐张核对过，所以按最严的一档处理：不主张这些图归我们、不把它们
用于任何商业发行、代码本身不依赖这些图才能跑（缺图就退方块面，再退色块）。要拿它们做
超出这个范围的事，请同时向 Mojang/Microsoft 和 Wiki 站方取授权。

其余 `assets/icons/` 下的字形（合计 873 个文件 / 579,411 B）来自上游
OpenNBS/OpenNoteBlockStudio 的 GameMaker 工程，按上游 `LICENSE`（**MIT**，
`Copyright (c) 2024 OpenNBS`；2019-01-21 首次提交那份是 `Copyright (c) 2019 Hielke`，
历史上只动过年份和作者名）分发。要说明的是：我们**没有逐帧核对过每一组字形内部有没有
引用 Mojang 像素**，上表只列出能确认的那几组；如果你发现别的目录也算衍生，请按同一
方式补进上表和无素材清单。

## 二、我们据以分发的那几句原文

摘自 [Minecraft EULA](https://www.minecraft.net/en-us/eula)：

> However, we retain ownership of our Minecraft intellectual property, which includes the
> game itself (including its content like the textures of our blocks and our character
> skins) and our Services, **and derivatives of each**.

> We will however own things that are copies (or substantial copies) or derivatives of our
> property and creations — but if you create original things, they aren't ours.

摘自 [Usage Guidelines for Fans and Creators](https://www.minecraft.net/en-us/usage-guidelines)：

> an original plug-in that extends the functionality of Minecraft but that **doesn't
> contain a substantial part of our copyrightable code or content**

> Do not redistribute our games or any alterations of our games or game files

读法（不是法律意见，只是我们自己的口径）：方块贴图及其衍生明确留在 Mojang 手里，
「mod 允许」那一条并不覆盖再分发他们的内容。我们入库的这 20 张 16×16、合计 4 KB，
是按 de minimis 和粉丝项目长期惯例在走，同时把来路、条款、退路都写清楚 —— 参照
[OpenNBS](https://github.com/OpenNBS/OpenNoteBlockStudio)（`datafiles/Data/instrumenttextures.zip`
整包随 MIT 仓库分发）、[mcmeta](https://github.com/misode/mcmeta)（把带贴图和不带贴图的
分支拆开 + 「Mojang 若有异议即刻移除」的原句）这类先例，也参照
[PrismLauncher](https://github.com/PrismLauncher/PrismLauncher)（库里一个 MC 素材都没有，
运行时现拉）这条最保守的路。我们选的是中间那档：**入 + 全标注 + 一条随时可切的无素材包**。

## 三、要完全干净的发行包

```
python tools/strip_mojang_assets.py --out dist/no-assets
```

它把 tracked 文件拷一份到 `dist/no-assets`，剔掉第一节表里列的那些 Mojang 素材，并在
产物里写一份 `NO-ASSETS.txt` 说明缺了什么、怎么补回来。缺图不缺功能：播放特效查一个方块
名先看这条特效要哪一层（`block_row`/`synthesia` 的「乐器方块」= 方块面 `assets/Blocks`，
`pan_slots` 的摞塔 = 等距图 `assets/BlocksIso`），首选那层没有再退另一层，两层都没有才按
名字哈希成稳定色块，`block_row` 的回执会说一句「没找到方块贴图，改用色块代替」。等距图那批
没有脚本可扒（Anubis），无素材包要补回来只能照 `assets/BlocksIso/PROVENANCE.json` 的原名
手动存图再改名。

## 四、第三方代码

| 项目 | 条款 | 在本仓库的形态 |
|---|---|---|
| [MiauNoteBlock](https://github.com/Miau-Studio/MiauNoteBlock) | AGPL-3.0-only（全文 `crates/nbs-io/MIAU-LICENSE`） | 移植自它的 MIDI 组合导入与声部处理：`crates/nbs-io/src/midi_miau.rs`、`midi_enhanced.rs`、`midi_voices.rs` 文件头各有一行 `Adapted from …; license: ../MIAU-LICENSE` |
| [MuScriptor](https://github.com/kichikuou/muscriptor) | MIT（全文 `crates/nbs-io/MUSCRIPTOR-LICENSE`） | 音频→MIDI 转录 worker：`crates/nbs-io/src/muscriptor_worker.py` |
| [Hyperchoron](https://github.com/Sheexio/Hyperchoron) / [er-mermaid-crafter](https://github.com/raphaeline/er-mermaid-crafter) | 抓取时未见 LICENSE | **不 vendor**。只在运行时按需调用外部副本，用户须自行取得并遵守其条款 |
| Rust 依赖（egui / eframe / rmcp / symphonia …） | 见各自 crate | 由 `Cargo.lock` 固定，不在这页逐条罗列 |

## 五、字体

`assets/fonts/SourceHanSansSC-Normal.otf`（16,525,388 B）：**SIL Open Font License 1.1**，
Adobe / Google / Sandoll 及贡献者。上游 `assets/credits.txt` 的 `*Fonts` 一节还列了
Liberation Sans、Noto Sans Symbols 2（同为 OFL 1.1）、Open Sans（Apache-2.0）、
WeblySleek UI（公有领域）；那些字体走装机素材，不在本仓库入库范围内。

## 六、下架与修改口径

Mojang 或任何权利人认为某一处不该出现在本仓库：直接提 issue。我们的处理顺序是
①按 `MOJANG-NOTICE.txt` / `WIKI-NOTICE.txt` 里写的那一步删白名单 / 删该路径，②代码不改
（缺素材的兜底本来就在：等距图整目录删掉会退到方块面，两层都缺才退色块），③在这一页记
一句「已按 X 要求移除」。改出厂乐器表时如果要新增方块贴图，`.gitignore` 的白名单和这条链
上的测试会一起提醒你把图补进第一节那张表（两个目录各一条测试）。

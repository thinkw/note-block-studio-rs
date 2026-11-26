"""产出一份不含 Mojang 素材的仓库副本（「无素材发行包」那一档）。

要剔的东西和口径都写在 THIRD-PARTY-NOTICES.md 第一节。名单只有一个来源：

* `assets/Blocks/` 与 `assets/BlocksIso/` 下各那 20 张（方块面 + Wiki 等距图）——
  直接读 `.gitignore` 里 `!assets/Blocks/<名>.png`、`!assets/BlocksIso/<名>.png`
  那几行，跟入库白名单是同一份，不会飘；
* `assets/Sounds/`、`assets/extranotes.zip`、以及 `assets/icons/` 里那六组
  明确是 Minecraft 内容衍生的精灵目录，写在下面的 `STRIP_PREFIXES` 里。

其余 `assets/icons/` 字形按上游 OpenNBS 的 MIT 分发，不剔（我们没逐帧核对过每组
字形内部有没有引用 Mojang 像素，见 THIRD-PARTY-NOTICES.md 第一节的说明）。

用法：

    python tools/strip_mojang_assets.py                 # 拷到 .work/no-assets
    python tools/strip_mojang_assets.py --out dist/no-assets --dry-run

产物只含 tracked 文件，未提交的改动不会跟进去（这本来就是发版口径）。
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# 除 Blocks 白名单之外，整棵目录或单个文件按 Mojang 素材对待。
STRIP_PREFIXES = [
    "assets/Sounds/",
    "assets/extranotes.zip",
    "assets/icons/block/",
    "assets/icons/block_color/",
    "assets/icons/block_color_shape/",
    "assets/icons/block_sel/",
    "assets/icons/block_shape/",
    "assets/icons/instrumenticons/",
]


def tracked_files() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "-z"], cwd=REPO_ROOT, capture_output=True, check=True
    ).stdout
    return [p.decode() for p in out.split(b"\0") if p]


def block_whitelist() -> list[str]:
    """`.gitignore` 里逐张放行的方块贴图 —— 和入库白名单同一份口径。

    两个目录都算：`assets/Blocks/`（16×16 方块面）与 `assets/BlocksIso/`
    （Wiki 的 300×300 等距图，只有声道方块的摞塔优先用这一层）。
    """
    ignore = (REPO_ROOT / ".gitignore").read_text(encoding="utf-8")
    out: list[str] = []
    for sub in ("assets/Blocks/", "assets/BlocksIso/"):
        prefix = "!" + sub
        out += [
            sub + line.strip()[len(prefix) :]
            for line in ignore.splitlines()
            if line.strip().startswith(prefix) and line.strip().endswith(".png")
        ]
    return out


def is_stripped(path: str, blocks: list[str]) -> bool:
    return path in blocks or any(path.startswith(p) for p in STRIP_PREFIXES)


def main() -> int:
    ap = argparse.ArgumentParser(description="产出不含 Mojang 素材的仓库副本")
    ap.add_argument("--out", default=str(REPO_ROOT / ".work" / "no-assets"),
                    help="落点目录（默认 .work/no-assets）")
    ap.add_argument("--dry-run", action="store_true", help="只统计，不拷贝")
    args = ap.parse_args()
    out = Path(args.out)

    files = tracked_files()
    blocks = block_whitelist()
    missing = [b for b in blocks if b not in files]
    if missing:
        raise SystemExit(f".gitignore 放行的贴图不在 git 索引里：{missing}")

    keep = [f for f in files if not is_stripped(f, blocks)]
    drop = [f for f in files if is_stripped(f, blocks)]
    dropped_bytes = sum((REPO_ROOT / f).stat().st_size for f in drop)

    print(f"tracked 总数 {len(files)}；剔 {len(drop)} 个（{dropped_bytes / 1024:.1f} KiB）；"
          f"留 {len(keep)} 个")
    print(f"  {'assets/Blocks/（.gitignore 白名单那几张）':38} "
          f"{len([b for b in blocks if b.startswith('assets/Blocks/')])}")
    print(f"  {'assets/BlocksIso/（等距图白名单那几张）':38} "
          f"{len([b for b in blocks if b.startswith('assets/BlocksIso/')])}")
    for prefix in STRIP_PREFIXES:
        n = len([f for f in drop if f.startswith(prefix)])
        print(f"  {prefix:38} {n}")
    if args.dry_run:
        print("dry-run，未落盘。")
        return 0

    if out.exists():
        shutil.rmtree(out)
    for f in keep:
        dst = out / f
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(REPO_ROOT / f, dst)

    (out / "NO-ASSETS.txt").write_text(
        "\n".join([
            "这份副本按「无素材」口径构建：不含 Mojang / Microsoft 的像素与音频。",
            f"构建时间 {time.strftime('%Y-%m-%dT%H:%M:%S')}",
            f"剔除 {len(drop)} 个文件（{dropped_bytes} 字节），保留 {len(keep)} 个。",
            "",
            "缺的东西以及怎么补回来：",
            "  assets/Blocks/*.png     跑 tools/fetch_block_textures.py（校验 jar sha1）",
            "  assets/BlocksIso/*.png  无脚本可扒（Wiki 有 Anubis 人机验证）：按",
            "                          assets/BlocksIso/PROVENANCE.json 里的原名存图再改名",
            "  assets/Sounds/*.ogg     取装机版 Minecraft Note Block Studio 的 Data\\Sounds",
            "  assets/extranotes.zip   同上，取 Data\\extranotes.zip",
            "  assets/icons/{block*,instrumenticons}/  上游 OpenNBS 工程的导出产物，",
            "                     见 assets/icons/SOURCE.txt 与 tools/export_sprites.py",
            "",
            "功能不受影响：每条特效按自己声明的那层优先查图（方块排与瀑布的「乐器方块」= 方块",
            "面，声道方块的摞塔 = 等距图），首选那层缺则退另一层，两层都缺退按名字哈希的稳定",
            "色块，素材全缺也画得出格子。",
            "口径全文见仓库根的 THIRD-PARTY-NOTICES.md、assets/Blocks/MOJANG-NOTICE.txt 与",
            "assets/BlocksIso/WIKI-NOTICE.txt。",
            "",
        ]) + "\n",
        encoding="utf-8",
    )
    print(f"已落 {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

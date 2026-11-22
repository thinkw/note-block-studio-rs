"""从 Mojang 官方客户端 jar 里抽方块贴图，落到 assets/Blocks/ 供播放特效用。

为什么不走 asset index：1.21.4 那份 index（id 19）只剩 sounds / lang / font，
`minecraft/textures` 一共 7 项，方块贴图全在客户端 jar 的
`assets/minecraft/textures/block/`（1039 张）里，所以只能下载 jar 再解包。

版本锁在 MC_VERSION；jar 的下载地址和 sha1 都从 version_manifest 现取现校验，
下载缓存放在 .work/mcassets/（不入库）。落盘的贴图保持 jar 里的原名，
`bone_block_side.png` / `bone_block_top.png` 这类多面方块由 Rust 侧按
「<name>.png → <name>_side.png → <name>_front.png → <name>_top.png」的顺序兜底，
工具这边不做名字归一，免得两处各有一套规则。

产物是 Mojang 的版权素材。仓库只入「20 个内置乐器真的会画到的那几张」（白名单在
.gitignore 里逐张 `!` 放行，名单由 crates/nbs-editor/src/fx/tiles.rs 那条测试对着
出厂表现算核对），其余 1000 多张跟乐器无关、不入库，只留在这台机器上。
随包分发（装进安装包）仍需另行拍板。
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import sys
import time
import urllib.request
import zipfile
from pathlib import Path

MC_VERSION = "1.21.4"
MANIFEST = "https://launchermeta.mojang.com/mc/game/version_manifest.json"
BLOCK_DIR = "assets/minecraft/textures/block/"
REPO_ROOT = Path(__file__).resolve().parent.parent


def http_get(url: str, timeout: int = 120) -> bytes:
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return r.read()


def client_download(version: str) -> tuple[str, str, int]:
    """返回 (url, sha1, size)。sha1 由 Mojang 自己给，我们只负责核对。"""
    manifest = json.loads(http_get(MANIFEST))
    hit = [v for v in manifest["versions"] if v["id"] == version]
    if not hit:
        raise SystemExit(f"版本清单里找不到 {version}；可用: "
                         f"{[v['id'] for v in manifest['versions'][:6]]}")
    meta = json.loads(http_get(hit[0]["url"]))
    d = meta["downloads"]["client"]
    return d["url"], d["sha1"], d["size"]


def jar_path(version: str, url: str, sha1: str, size: int, cache: Path) -> Path:
    cache.mkdir(parents=True, exist_ok=True)
    p = cache / f"{version}-client.jar"
    if p.exists():
        got = hashlib.sha1(p.read_bytes()).hexdigest()
        if got == sha1:
            print(f"缓存命中 {p}（sha1 已核对）")
            return p
        print(f"缓存里那份 sha1 不对（{got[:12]}），重新下载")
        p.unlink()
    print(f"下载客户端 jar：{size / 1e6:.1f} MB …")
    data = http_get(url, timeout=600)
    got = hashlib.sha1(data).hexdigest()
    if got != sha1:
        raise SystemExit(f"sha1 不符：期望 {sha1}，得到 {got}")
    p.write_bytes(data)
    print(f"已存 {p}（sha1 核对通过）")
    return p


def extract(jar: Path, out: Path) -> tuple[int, int]:
    with zipfile.ZipFile(jar) as z:
        names = [n for n in z.namelist() if n.startswith(BLOCK_DIR) and n.endswith(".png")]
        out.mkdir(parents=True, exist_ok=True)
        total = 0
        for n in names:
            data = z.read(n)
            (out / Path(n).name).write_bytes(data)
            total += len(data)
        return len(names), total


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--version", default=MC_VERSION, help=f"锁定版本（默认 {MC_VERSION}）")
    ap.add_argument("--out", default=str(REPO_ROOT / "assets" / "Blocks"),
                    help="落盘目录（默认 assets/Blocks）")
    ap.add_argument("--cache", default=str(REPO_ROOT / ".work" / "mcassets"),
                    help="jar 缓存目录（默认 .work/mcassets，不入库）")
    args = ap.parse_args()

    url, sha1, size = client_download(args.version)
    jar = jar_path(args.version, url, sha1, size, Path(args.cache))
    out = Path(args.out)
    count, bytes_out = extract(jar, out)

    (out / "PROVENANCE.json").write_text(json.dumps({
        "source": "Mojang Studios 官方客户端 jar，路径 assets/minecraft/textures/block/",
        "minecraft_version": args.version,
        "jar_sha1": sha1,
        "jar_size": size,
        "textures": count,
        "bytes": bytes_out,
        "fetched_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "fetched_by": "tools/fetch_block_textures.py",
        "license_note": "Mojang/Microsoft 版权素材；仓库只入 20 个内置乐器用到的那几张"
                        "（白名单见 .gitignore，名单由 fx::tiles 的测试现算核对），"
                        "其余不入库；随包分发需另行拍板。",
    }, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    print(f"解出 {count} 张（{bytes_out / 1e6:.1f} MB）→ {out}")
    print(f"来路记在 {out / 'PROVENANCE.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

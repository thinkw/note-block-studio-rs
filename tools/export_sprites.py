"""从原版 GameMaker 工程里导出精灵帧，落到 nbs-rust 的 assets/icons/<集合>/<序号>.png。

序号就是 GML 里 draw_sprite(sprite, <序号>, ...) 的 subimg，顺序取自
sprites/<name>/<name>.yy 的 frames 数组（GM 写的 JSON 有尾逗号，先剥掉再解析）。

精灵资源是按版本改的，导出必须锁定在装机版对应的 tag 上（REF）。
默认走 `git archive <REF> sprites`，把那个 tag 的 sprites/ 解到 target/ 下再读，
这样即使工作树停在别的提交上也不会导出错的贴图。

帧图有两种工程布局，都支持：
  新版（>= 2022.0.3，装机版）：frames 只有 name，合成好的图就在
    sprites/<name>/<frame uuid>.png（多层精灵时这张就是各层叠加后的结果）；
    万一没有，再退回 sprites/<name>/layers/<frame uuid>/<layer uuid>.png 自己叠。
  旧版：frames 里带 images 数组，需要按图层顺序 alpha-over 合成。

用法：
    python tools/export_sprites.py <精灵名> [<精灵名> ...]
    python tools/export_sprites.py --all
    python tools/export_sprites.py --list
    python tools/export_sprites.py --check        # 只比对，不写盘
"""

import io
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import zlib

SRC_REPO = os.environ.get("OPENNBS_DIR", r"D:\develop\rust-project\NoteBlockStudio")
REF = os.environ.get("OPENNBS_REF", "v3.12.0-rc.1")
DST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "assets", "icons")

# 装机版在用的精灵，一次导全。目录名 = 精灵名去掉 spr_ 前缀（SOURCE.txt 里有改名说明）。
ALL_SPRITES = [
    "spr_back", "spr_block", "spr_block_sel", "spr_block_color", "spr_block_color_shape",
    "spr_block_shape", "spr_button_arrow", "spr_frame1", "spr_frame4", "spr_iconbar",
    "spr_icons", "spr_icons_col", "spr_icons_d", "spr_icons_f", "spr_instrumenticons",
    "spr_layerbox", "spr_layericons", "spr_layericons_f", "spr_lock", "spr_macroicons",
    "spr_marker", "spr_minecraft", "spr_minecraft_f", "spr_numbers", "spr_numbers_mc",
    "spr_piano", "spr_stereo", "spr_stereo_f", "spr_tempobox", "spr_volume", "spr_volume_f",
    "spr_yesno", "spr_yesno_f",
    # 消音器导引带 + 事件乐器的角标：spr_wall 原点 x=1，画的时候要先左移 1px 再裁剪。
    "spr_tempo_changer_label", "spr_tempo_changer_label_mc", "spr_wall",
    # 标尺 BPM 盒的边框（2x2、每主题 8 帧，先角后边）。此前窗口边框一直用主题描边近似，
    # 这里是唯一像素级还原的地方，一并导出。
    "spr_interface1",
]


# 精灵名 -> assets/icons/ 下的目录名。多数就是去掉 spr_ 前缀，
# 少数四套 UI 字形按主题命名，导出时改过名，别重新落回精灵名。
DIRS = {
    "spr_icons": "icons_classic",
    "spr_icons_f": "icons_light",
    "spr_icons_d": "icons_d",
    "spr_icons_col": "icons_accent",
    "spr_layericons": "layer_classic",
    "spr_layericons_f": "layer_fluent",
    "spr_macroicons": "macro",
    "spr_minecraft_f": "minecraft_fluent",
    "spr_stereo_f": "stereo_fluent",
    "spr_volume_f": "volume_fluent",
}


def load_yy(path):
    text = io.open(path, encoding="utf-8").read()
    return json.loads(re.sub(r",(\s*[}\]])", r"\1", text))


def materialize():
    """把 REF 那个 tag 的 sprites/ 解出来，返回它的根目录。"""
    root = os.path.join(os.path.dirname(DST), "target", "gm-sprites-" + REF)
    if not os.path.isdir(os.path.join(root, "sprites")):
        os.makedirs(root, exist_ok=True)
        archive = subprocess.run(["git", "-C", SRC_REPO, "archive", REF, "sprites"],
                                 stdout=subprocess.PIPE, check=True).stdout
        subprocess.run(["tar", "-x", "-C", root], input=archive, check=True)
    return root


def png_decode(path):
    """解出 RGBA8 画布。只覆盖本工程用到的编码：8 位真彩 / 灰度，带或不带 alpha。"""
    data = open(path, "rb").read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", path
    pos = 8
    idat = bytearray()
    w = h = depth = color = None
    while pos < len(data):
        (ln,) = struct.unpack(">I", data[pos:pos + 4])
        kind = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + ln]
        pos += 12 + ln
        if kind == b"IHDR":
            w, h, depth, color = struct.unpack(">IIBB", body[:10])
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
    raw = zlib.decompress(bytes(idat))
    channels = {0: 1, 2: 3, 4: 2, 6: 4}[color]
    assert depth == 8, f"{path}: 位深 {depth} 不支持"
    bpp = channels
    stride = w * bpp
    out = bytearray(w * h * 4)
    prev = bytearray(stride)
    ipos = 0
    for y in range(h):
        ftype = raw[ipos]
        ipos += 1
        line = bytearray(raw[ipos:ipos + stride])
        ipos += stride
        for i in range(stride):
            a = line[i - bpp] if i >= bpp else 0
            b = prev[i]
            c = prev[i - bpp] if i >= bpp else 0
            if ftype == 1:
                line[i] = (line[i] + a) & 0xFF
            elif ftype == 2:
                line[i] = (line[i] + b) & 0xFF
            elif ftype == 3:
                line[i] = (line[i] + ((a + b) >> 1)) & 0xFF
            elif ftype == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        prev = line
        for x in range(w):
            o = (y * w + x) * 4
            s = x * bpp
            if channels == 4:
                out[o:o + 4] = line[s:s + 4]
            elif channels == 3:
                out[o:o + 3] = line[s:s + 3]
                out[o + 3] = 255
            elif channels == 2:
                g = line[s]
                out[o:o + 3] = bytes([g, g, g])
                out[o + 3] = line[s + 1]
            else:
                g = line[s]
                out[o:o + 3] = bytes([g, g, g])
                out[o + 3] = 255
    return w, h, out


def png_encode(path, w, h, rgba):
    def chunk(kind, body):
        return (struct.pack(">I", len(body)) + kind + body
                + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF))
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        raw += rgba[y * w * 4:(y + 1) * w * 4]
    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    out += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    out += chunk(b"IEND", b"")
    if path is None:
        return out
    open(path, "wb").write(out)
    return out


def composite(width, height, layers):
    """layers 自下而上，每层 (x, y, w, h, rgba)。"""
    out = bytearray(width * height * 4)
    for (ox, oy, lw, lh, buf) in layers:
        for y in range(lh):
            ty = oy + y
            if ty < 0 or ty >= height:
                continue
            for x in range(lw):
                tx = ox + x
                if tx < 0 or tx >= width:
                    continue
                so = (y * lw + x) * 4
                a = buf[so + 3]
                if a == 0:
                    continue
                do = (ty * width + tx) * 4
                if a == 255:
                    out[do:do + 4] = buf[so:so + 4]
                else:
                    da = out[do + 3]
                    na = a + da * (255 - a) // 255
                    for c in range(3):
                        sc, dc = buf[so + c], out[do + c]
                        out[do + c] = (sc * a + dc * (255 - a) * da // 255) // na if na else 0
                    out[do + 3] = na
    return out


def frame_png(src, name, fr):
    """取一帧的合成图路径；旧布局（帧里带 images）返回 None，由调用方自己叠图层。"""
    p = os.path.join(src, "sprites", name, fr["name"] + ".png")
    return p if os.path.exists(p) else None


def export(name, src, check=False):
    yy = load_yy(os.path.join(src, "sprites", name, name + ".yy"))
    width, height = yy["width"], yy["height"]
    frames = yy["frames"]
    dst = os.path.join(DST, DIRS.get(name, name.replace("spr_", "")))
    if not check:
        os.makedirs(dst, exist_ok=True)
    stale = []
    for i, fr in enumerate(frames):
        p = frame_png(src, name, fr)
        if p is None:
            layers = []
            for im in reversed(fr["images"]):
                q = os.path.join(src, "sprites", name, "layers", fr["name"],
                                 im["LayerId"]["name"] + ".png")
                if os.path.exists(q):
                    lw, lh, buf = png_decode(q)
                    layers.append((0, 0, lw, lh, buf))
            rgba = composite(width, height, layers) if layers else bytearray(width * height * 4)
            out = png_encode(None, width, height, rgba)
        else:
            lw, lh, buf = png_decode(p)
            assert (lw, lh) == (width, height), f"{name} 帧 {i} 尺寸 {lw}x{lh}，精灵是 {width}x{height}"
            rgba = buf
            out = png_encode(None, width, height, rgba)
        fn = os.path.join(dst, f"{i:02d}.png")
        if check:
            # 只比像素：重新压缩出来的字节和已提交的文件未必逐字节相同，那不是版本差异。
            if not os.path.exists(fn):
                stale.append(i)
            else:
                ow, oh, obuf = png_decode(fn)
                if (ow, oh) != (width, height) or bytes(obuf) != bytes(rgba):
                    stale.append(i)
            continue
        open(fn, "wb").write(out)
    if check:
        extra = len(os.listdir(dst)) - len(frames) if os.path.isdir(dst) else 0
        bad = stale + (["多出 %d 帧" % extra] if extra > 0 else [])
        print("%-24s %s" % (name, "一致" if not bad else "需要重导：%s" % (bad,)))
        return not bad
    print(f"{name}: {len(frames)} 帧 {width}x{height} -> {dst}")
    return True


def main():
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    argv = sys.argv[1:]
    if not argv:
        print(__doc__)
        return
    src = materialize()
    print("精灵来源：%s @ %s" % (SRC_REPO, REF))
    if argv[0] == "--list":
        for n in sorted(os.listdir(os.path.join(src, "sprites"))):
            yy = os.path.join(src, "sprites", n, n + ".yy")
            if os.path.exists(yy):
                j = load_yy(yy)
                print("%-30s %3d 帧  %sx%s" % (n, len(j["frames"]), j["width"], j["height"]))
        return
    if argv[0] == "--all":
        argv = ALL_SPRITES
    check = argv[0] == "--check"
    if check:
        argv = ALL_SPRITES
    results = [export(n, src, check) for n in argv]
    ok = all(results)
    if check:
        print("全部一致" if ok else "有精灵不是 %s 的版本，跑 python tools/export_sprites.py --all" % REF)
        sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()

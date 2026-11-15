"""从原版 GameMaker 工程里导出精灵帧，落到 nbs-rust 的 assets/icons/<集合>/<序号>.png。

序号就是 GML 里 draw_sprite(sprite, <序号>, ...) 的 subimg，顺序取自
sprites/<name>/<name>.yy 的 frames 数组（GM 写的 JSON 有尾逗号，先剥掉再解析）。
每帧若有多个图层，按图层顺序以标准 alpha-over 合成到一张画布上。

用法：
    python tools/export_sprites.py <精灵名> [<精灵名> ...]
    python tools/export_sprites.py --list
"""

import io
import json
import os
import re
import struct
import sys
import zlib

SRC = os.environ.get("OPENNBS_DIR", r"D:\develop\rust-project\NoteBlockStudio")
DST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "assets", "icons")


def load_yy(path):
    text = io.open(path, encoding="utf-8").read()
    return json.loads(re.sub(r",(\s*[}\]])", r"\1", text))


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
    open(path, "wb").write(out)


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


def export(name):
    yy = load_yy(os.path.join(SRC, "sprites", name, name + ".yy"))
    width, height = yy["width"], yy["height"]
    frames = yy["frames"]
    dst = os.path.join(DST, name.replace("spr_", ""))
    os.makedirs(dst, exist_ok=True)
    for i, fr in enumerate(frames):
        layers = []
        for im in reversed(fr["images"]):
            p = os.path.join(SRC, "sprites", name, "layers", fr["name"], im["LayerId"]["name"] + ".png")
            if not os.path.exists(p):
                continue
            lw, lh, buf = png_decode(p)
            layers.append((0, 0, lw, lh, buf))
        rgba = composite(width, height, layers) if layers else bytearray(width * height * 4)
        png_encode(os.path.join(dst, f"{i:02d}.png"), width, height, rgba)
    print(f"{name}: {len(frames)} 帧 {width}x{height} -> {dst}")


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return
    if sys.argv[1] == "--list":
        for n in sorted(os.listdir(os.path.join(SRC, "sprites"))):
            yy = os.path.join(SRC, "sprites", n, n + ".yy")
            if os.path.exists(yy):
                j = load_yy(yy)
                print("%-30s %3d 帧  %sx%s" % (n, len(j["frames"]), j["width"], j["height"]))
        return
    for n in sys.argv[1:]:
        export(n)


if __name__ == "__main__":
    main()

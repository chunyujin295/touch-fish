# 由 docs/img/logo2.png 生成应用图标。
#
#   python scripts/make_icon.py
#
# 产出：
#   assets/app/icon.rgba   64×64 裸 RGBA，给 tao 当窗口图标、给 tray-icon 当托盘图标
#   assets/app/icon.png    64×64 PNG，留给需要图片格式的地方
#   assets/app/logo.png   128×128 PNG，书库界面的 favicon 与左上角品牌图标
#
# 不引 Pillow：源图是 8 位 RGB、非隔行，标准滤波就那五种，自己解一遍反而没有依赖。
# 只做两件事：① 裁掉四周大片留白 ② 用面积平均缩放（不是最近邻）。
# 刻意**不做**透明背景：原图什么底就什么底，图标通道固定 255。

import struct
import sys
import zlib

SRC = "docs/img/logo2.png"
ICON = 64
LOGO = 128


def read_png(path):
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit("不是 PNG")
    pos = 8
    w = h = None
    bit = color = interlace = None
    idat = bytearray()
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        tag = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        if tag == b"IHDR":
            w, h, bit, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
        elif tag == b"IDAT":
            idat += body
        elif tag == b"IEND":
            break
        pos += 12 + length
    if bit != 8 or color not in (2, 6) or interlace != 0:
        raise SystemExit("只处理 8 位、非隔行的 RGB / RGBA")
    bpp = 3 if color == 2 else 4
    raw = zlib.decompress(bytes(idat))

    stride = w * bpp
    out = bytearray(stride * h)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        f = raw[p]
        p += 1
        line = bytearray(raw[p : p + stride])
        p += stride
        if f == 1:  # Sub
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif f == 2:  # Up
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif f == 3:  # Average
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif f == 4:  # Paeth
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        elif f != 0:
            raise SystemExit("未知滤波类型 %d" % f)
        out[y * stride : (y + 1) * stride] = line
        prev = line
    return w, h, bpp, bytes(out)


def to_rows(w, h, bpp, buf):
    """统一成 (r,g,b,a) 四元组的二维表，RGB 源补满不透明"""
    rows = []
    for y in range(h):
        row = []
        base = y * w * bpp
        for x in range(w):
            i = base + x * bpp
            if bpp == 3:
                row.append((buf[i], buf[i + 1], buf[i + 2], 255))
            else:
                row.append((buf[i], buf[i + 1], buf[i + 2], buf[i + 3]))
        rows.append(row)
    return rows


def content_box(rows, w, h):
    """找出真正的作画范围，把四周大片白边裁掉"""
    left, top, right, bottom = w, h, -1, -1
    for y in range(h):
        row = rows[y]
        for x in range(w):
            r, g, b, _ = row[x]
            if min(r, g, b) < 240:
                if x < left:
                    left = x
                if x > right:
                    right = x
                if y < top:
                    top = y
                if y > bottom:
                    bottom = y
    if right < 0:
        raise SystemExit("图里没有找到内容")
    return left, top, right, bottom


def square_with_padding(box, w, h, pad_ratio=0.06):
    left, top, right, bottom = box
    side = max(right - left + 1, bottom - top + 1)
    side = int(side * (1 + pad_ratio * 2))
    cx = (left + right) // 2
    cy = (top + bottom) // 2
    x0 = cx - side // 2
    y0 = cy - side // 2
    return x0, y0, side


def opaque(px):
    """alpha 固定 255（不做透明背景）；接近纯白的底色统一压成纯白。

    源图是从带压缩痕迹的图转来的，白底上有 250~254 的杂色，
    缩到 64px 会变成一片脏点，所以这里把「几乎白」直接归白。
    """
    r, g, b, _ = px
    if min(r, g, b) >= 248:
        return (255, 255, 255, 255)
    return (r, g, b, 255)


def resize(rows, x0, y0, side, size):
    """面积平均缩放：每个目标像素取源图对应区块的平均色，alpha 恒为 255"""
    out = []
    scale = side / size
    for oy in range(size):
        line = []
        sy0 = y0 + int(oy * scale)
        sy1 = max(sy0 + 1, y0 + int((oy + 1) * scale))
        for ox in range(size):
            sx0 = x0 + int(ox * scale)
            sx1 = max(sx0 + 1, x0 + int((ox + 1) * scale))
            rs = gs = bs = n = 0
            for sy in range(sy0, sy1):
                for sx in range(sx0, sx1):
                    r, g, b, _ = opaque(rows[sy][sx])
                    rs += r
                    gs += g
                    bs += b
                    n += 1
            line.append((rs // n, gs // n, bs // n, 255))
        out.append(line)
    return out


def write_png(path, rows, size):
    """输出 RGB（颜色类型 2）而不是 RGBA：文件里根本不带 alpha 通道"""
    raw = bytearray()
    for y in range(size):
        raw.append(0)
        for x in range(size):
            r, g, b, _ = rows[y][x]
            raw.extend((r, g, b))

    def chunk(tag, body):
        return (
            struct.pack(">I", len(body))
            + tag
            + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
        )

    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )
    open(path, "wb").write(png)
    return len(png)


def write_rgba(path, rows, size):
    buf = bytearray()
    for y in range(size):
        for x in range(size):
            buf.extend(rows[y][x])
    open(path, "wb").write(buf)
    return len(buf)


if __name__ == "__main__":
    w, h, bpp, buf = read_png(SRC)
    rows = to_rows(w, h, bpp, buf)
    box = content_box(rows, w, h)
    x0, y0, side = square_with_padding(box, w, h)
    x0 = max(0, min(x0, w - side))
    y0 = max(0, min(y0, h - side))
    side = min(side, w - x0, h - y0)
    print("源图 %dx%d，内容框 %s，裁成 %d×%d 的方图" % (w, h, box, side, side))

    icon = resize(rows, x0, y0, side, ICON)
    logo = resize(rows, x0, y0, side, LOGO)
    n1 = write_rgba("assets/app/icon.rgba", icon, ICON)
    n2 = write_png("assets/app/icon.png", icon, ICON)
    n3 = write_png("assets/app/logo.png", logo, LOGO)
    print("assets/app/icon.rgba %d 字节（%d×%d 裸 RGBA）" % (n1, ICON, ICON))
    print("assets/app/icon.png  %d 字节" % n2)
    print("assets/app/logo.png  %d 字节（%d×%d）" % (n3, LOGO, LOGO))

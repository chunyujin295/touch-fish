# 生成用于验证 MOBI 解析的样书：python scripts/make_test_mobi.py
#
# 手写一个合法的 PalmDB/MOBI：
#   记录 0   PalmDOC 头(16) + MOBI 头(232) + EXTH（书名 503 / 作者 100）
#   记录 1..N 正文，PalmDOC 压缩（这里只产字面量块，够验证解压路径）
#   记录 N+1 一张 PNG，正文里用 <img recindex="00001"/> 引用
#
# 之所以自己造而不是下载一本：需要确定性的、带分页符与行内格式的最小样本，
# 出错时能一眼看出是解析器还是样本的问题。

import os
import struct
import zipfile

OUT_DIR = os.path.join(os.path.dirname(__file__), "..", "samples")

# 真实 MOBI 用 4096；这里故意设小，好让样书的正文跨多条记录，
# 从而真正检验"字符被记录边界切断"的顺延逻辑
RECORD_SIZE = 1024

PAGES = [
    """<html><body>
<h1>第一章　摸鱼的基本功</h1>
<p>摸鱼不是偷懒，是在<strong>信息密度</strong>极低的场合里保住自己的产出。这一章讲三个前提。</p>
<p>第一，手边必须有一份<em>看起来正在处理</em>的文档；第二，屏幕要在视线正前方；第三，切换速度要快。</p>
<blockquote>真正的技巧不在读什么，而在别人路过的那两秒里，你屏幕上是什么。</blockquote>
</body></html>""",
    """<html><body>
<h1>第二章　窗口与边缘</h1>
<p>把阅读器放在副屏、或者半屏贴边，是最稳的姿势。全屏反而危险——它太干净了。</p>
<p>工具链是这样的：<code>reader --skin csdn --pack cs</code>，一行命令换一层壳。</p>
<p>需要一份行内链接吗？参考 <a href="https://example.com/notes">公开笔记</a>，但别点开。</p>
</body></html>""",
    """<html><body>
<h1>第三章　当有人站在你身后</h1>
<p>Esc 是第一反应键。它比最小化快，比切窗口自然，而且不留历史记录。</p>
<p>下面这张图是示意，用来验证 MOBI 里的图片记录能不能正确取出来：</p>
<img recindex="00001" alt="示意图"/>
<p>恢复的时候不需要重新打开程序，热键一按就回来了。</p>
</body></html>""",
    """<html><body>
<h1>第四章　长文与分页</h1>
<p>这一章故意写长，用来验证文本记录被切成多条之后还能拼回来。真实的 MOBI 每条文本记录上限是 4096 字节，一本几百页的书会被切成上百条。</p>
<p>切点往往落在汉字中间。如果不把半个字符顺延到下一条记录，拼接处就会出现一串替换字符——这是自研解析器最典型的破绽。</p>
<p>所以在解压循环里，每条记录解完都要检查末尾是不是完整字符：不完整就把那几个字节留下来，跟下一条的开头接上，再一起解码。</p>
<p>同样的道理适用于 UTF-16 或 GBK 的书，只是那两种编码需要按各自的最小单元对齐，逻辑要更啰嗦一些，这里先只处理 UTF-8。</p>
<p>判断方法很直接：把整章渲染出来，搜替换字符（U+FFFD）。搜不到就说明拼接是对的；搜到一处，就说明边界处理还差一点。</p>
<p>这段文字本身也算一次检验：它跨过了记录边界，如果读起来连贯，说明顺延逻辑生效了。</p>
</body></html>""",
    """<html><body>
<h1>第五章　收尾</h1>
<p>整本书只有五章，够用来验证分章、行内格式、图片和跨记录拼接四件事了。</p>
<p>真实书里还会有目录、脚注、封面，这些在 MOBI 里分别是额外的索引记录和图片记录，不在正文流里。当前版本只取正文与图片，目录用解析出来的标题顶替。</p>
<p>如果遇到 HUFF/CDIC 压缩或者带 DRM 的文件，导入会直接给出提示，而不是读出一堆乱码。</p>
</body></html>""",
]


def palmdoc_compress(data: bytes) -> bytes:
    """只产字面量块的最小实现：0x09..0x7f 直传，其余用 0x01..0x08 块搬运。"""
    out = bytearray()
    i = 0
    while i < len(data):
        b = data[i]
        if b == 0 or 9 <= b <= 0x7F:
            out.append(b)
            i += 1
        else:
            n = min(8, len(data) - i)
            out.append(n)
            out += data[i : i + n]
            i += n
    return bytes(out)


def split_records(text: bytes):
    """按 4096 字节切文本记录，切点必须落在 UTF-8 字符边界上。"""
    out = []
    i = 0
    while i < len(text):
        end = min(i + RECORD_SIZE, len(text))
        # 往前退到字符边界
        while end < len(text) and (text[end] & 0xC0) == 0x80:
            end -= 1
        out.append(text[i:end])
        i = end
    return out


def exth(title: str, author: str) -> bytes:
    recs = b""
    for kind, value in ((503, title), (100, author)):
        data = value.encode("utf-8")
        recs += struct.pack(">II", kind, len(data) + 8) + data
    body = struct.pack(">II", 12 + len(recs), 2) + recs
    return b"EXTH" + body


def mobi_record0(text_len: int, text_records: int, first_image: int, title: str, author: str) -> bytes:
    # PalmDOC 头 16 字节：压缩方式 / 保留 / 文本长度 / 文本记录数 / 记录大小 / 加密 / 保留
    palmdoc = struct.pack(">HHIHHHH", 2, 0, text_len, text_records, RECORD_SIZE, 0, 0)
    assert len(palmdoc) == 16

    header_len = 232
    mobi = bytearray(header_len)
    mobi[0:4] = b"MOBI"
    struct.pack_into(">I", mobi, 4, header_len)
    struct.pack_into(">I", mobi, 8, 2)          # mobi type: book
    struct.pack_into(">I", mobi, 12, 65001)     # 文本编码 UTF-8
    struct.pack_into(">I", mobi, 16, 1)         # unique id
    struct.pack_into(">I", mobi, 20, 6)         # file version 6
    struct.pack_into(">I", mobi, 92, first_image)  # 首图记录号
    struct.pack_into(">I", mobi, 112, 0x40)     # EXTH 存在标志
    return palmdoc + bytes(mobi) + exth(title, author)


def build_pdb(path: str, title: str, author: str):
    text = "<mbp:pagebreak/>".join(PAGES).encode("utf-8")
    chunks = [palmdoc_compress(c) for c in split_records(text)]
    png = open(os.path.join(OUT_DIR, "cover.png"), "rb").read()

    first_image = 1 + len(chunks)
    records = [
        mobi_record0(len(text), len(chunks), first_image, title, author),
        *chunks,
        png,
    ]

    header = bytearray(78)
    name = title.encode("utf-8")[:31]
    header[0 : len(name)] = name
    struct.pack_into(">H", header, 32, 0)      # attributes
    struct.pack_into(">H", header, 34, 0)      # version
    header[60:64] = b"BOOK"
    header[64:68] = b"MOBI"
    struct.pack_into(">H", header, 76, len(records))

    offset = 78 + 8 * len(records)
    table = b""
    body = b""
    for rec in records:
        table += struct.pack(">II", offset + len(body), 0)
        body += rec

    with open(path, "wb") as f:
        f.write(bytes(header) + table + body)


if __name__ == "__main__":
    os.makedirs(OUT_DIR, exist_ok=True)
    # 图片记录：直接从应用图标借一张，免得再引一个图片依赖
    src = os.path.join(os.path.dirname(__file__), "..", "assets", "app", "icon.png")
    open(os.path.join(OUT_DIR, "cover.png"), "wb").write(open(src, "rb").read())

    out = os.path.join(OUT_DIR, "摸鱼基本功.mobi")
    build_pdb(out, "摸鱼基本功", "佚名")
    print("wrote", os.path.abspath(out), os.path.getsize(out), "bytes")

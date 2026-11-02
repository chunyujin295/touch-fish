# 生成用于本地验证的样书（EPUB 3 + TXT）。
# 只是开发用工具，不参与打包：python tools/make_test_book.py

import os
import zipfile

OUT_DIR = os.path.join(os.path.dirname(__file__), "..", "samples")

CHAPTERS = [
    (
        "第一章　从一次跨机房延迟说起",
        [
            "凌晨两点，监控上看不出任何异常，QPS 没有涨，写入没有变，CPU 也很平静。唯一的信号是 P99 从 8ms 抬到了 60ms，持续了 17 分钟，然后自己好了。",
            "这类故障最麻烦的地方在于，它不发生在上线的时刻，也不伴随任何报警。等你打开看板，它已经过去了，只留下一条锯齿状的曲线。",
            "排查的第一步永远是排除最无聊的原因：机器是不是被别的任务抢了 CPU，网络是不是有丢包，日志是不是把磁盘写满了。这三条排除之后，才轮到协议与一致性的问题。",
        ],
    ),
    (
        "第二章　Raft 的读路径为什么比写路径更难",
        [
            "写入的路径是清晰的：Leader 收到请求，追加日志，复制到多数派，提交，返回。每一步都有明确的状态转移，出问题时日志里一定留着痕迹。",
            "读就麻烦了。如果允许任意 Follower 直接读本地状态机，那读到的东西可能落后很久，甚至可能读到一个已经被推翻的旧 Leader 的数据，这就是著名的脏读问题。",
            "ReadIndex 的思路是：先向 Leader 要一个凭证，证明「在这个时刻我是 Leader，并且我已提交到第 N 条」，然后 Follower 至少等到本地应用到第 N 条，才允许读。",
            "难点在于「等」这件事。等得太粗，延迟会抖；等得太细，唤醒的开销会吃掉收益。",
        ],
    ),
    (
        "第三章　把通知当成条件变量是常见的错误",
        [
            "很多实现里，apply 协程会在提交成功后广播一个信号，所有等待读的协程被唤醒，然后重新比较自己的水位。这在语义上是「通知」，不是「条件变量」——通知是有损的，唤醒不等于条件成立。",
            "当批量提交把 512 条日志当成一次提交时，这个损失会被放大：等待方被唤醒，发现还差三条，只能再等下一批。跨机房一次往返 30ms，抖动就是这么来的。",
            "修法有两种。一种是缩小批大小，代价是写吞吐下降；另一种是给每个等待方挂一个按索引排序的最小堆，apply 完成后只唤醒真正满足条件的那几个。",
            "我们选了后者。压测下 CPU 多用了 2%，P99 从 60ms 回到 11ms。这个交换是划算的。",
        ],
    ),
]


def xhtml(title, paras):
    body = "\n".join(f"    <p>{p}</p>" for p in paras)
    return f"""<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="zh-CN">
<head><meta charset="utf-8"/><title>{title}</title>
<style>body{{font-family:serif;line-height:1.8}}</style></head>
<body>
    <h1>{title}</h1>
{body}
</body>
</html>
"""


def build_epub(path):
    container = """<?xml version="1.0" encoding="utf-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"""

    items = []
    spine = []
    for i, (title, _) in enumerate(CHAPTERS, start=1):
        items.append(
            f'    <item id="ch{i}" href="Text/ch{i}.xhtml" media-type="application/xhtml+xml"/>'
        )
        spine.append(f'    <itemref idref="ch{i}"/>')

    opf = f"""<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:uuid:tf-test-0001</dc:identifier>
    <dc:title>分布式系统手记</dc:title>
    <dc:creator>林一</dc:creator>
    <dc:language>zh-CN</dc:language>
    <dc:description>一本用于验证排版与解析的虚构技术书。</dc:description>
    <meta property="dcterms:modified">2026-10-30T12:00:00Z</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
{chr(10).join(items)}
  </manifest>
  <spine toc="ncx">
{chr(10).join(spine)}
  </spine>
</package>
"""

    nav_lis = "\n".join(
        f'      <li><a href="Text/ch{i}.xhtml">{t}</a></li>'
        for i, (t, _) in enumerate(CHAPTERS, start=1)
    )
    nav = f"""<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><meta charset="utf-8"/><title>目录</title></head>
<body><nav epub:type="toc" id="toc"><h1>目录</h1><ol>
{nav_lis}
</ol></nav></body>
</html>
"""

    points = "\n".join(
        f"""    <navPoint id="np{i}" playOrder="{i}">
      <navLabel><text>{t}</text></navLabel>
      <content src="Text/ch{i}.xhtml"/>
    </navPoint>"""
        for i, (t, _) in enumerate(CHAPTERS, start=1)
    )
    ncx = f"""<?xml version="1.0" encoding="utf-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head><meta name="dtb:uid" content="urn:uuid:tf-test-0001"/></head>
  <docTitle><text>分布式系统手记</text></docTitle>
  <navMap>
{points}
  </navMap>
</ncx>
"""

    with zipfile.ZipFile(path, "w") as z:
        # mimetype 必须第一个写入且不压缩
        z.writestr(zipfile.ZipInfo("mimetype"), "application/epub+zip", compress_type=zipfile.ZIP_STORED)
        z.writestr("META-INF/container.xml", container, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("OEBPS/content.opf", opf, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("OEBPS/nav.xhtml", nav, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("OEBPS/toc.ncx", ncx, compress_type=zipfile.ZIP_DEFLATED)
        for i, (title, paras) in enumerate(CHAPTERS, start=1):
            z.writestr(
                f"OEBPS/Text/ch{i}.xhtml",
                xhtml(title, paras),
                compress_type=zipfile.ZIP_DEFLATED,
            )


def build_txt(path):
    lines = ["《分布式系统手记》 作者：林一", ""]
    for title, paras in CHAPTERS:
        lines.append(title)
        lines.append("")
        for p in paras:
            lines.append(p)
            lines.append("")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))


if __name__ == "__main__":
    os.makedirs(OUT_DIR, exist_ok=True)
    epub = os.path.join(OUT_DIR, "分布式系统手记.epub")
    txt = os.path.join(OUT_DIR, "分布式系统手记.txt")
    build_epub(epub)
    build_txt(txt)
    print("wrote", os.path.abspath(epub), os.path.getsize(epub), "bytes")
    print("wrote", os.path.abspath(txt), os.path.getsize(txt), "bytes")

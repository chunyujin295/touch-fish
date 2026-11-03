# 生成用于本地验证的样书（EPUB 3 + TXT）。
# 只是开发用工具，不参与打包：python scripts/make_test_book.py

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
            "读就麻烦了。如果允许任意 Follower 直接读本地状态机，那读到的东西可能落后很久，甚至可能读到一个已经被推翻的旧 Leader 的数据，这就是著名的<b>脏读</b>问题。",
            "ReadIndex 的思路是：先向 Leader 要一个凭证，证明「在这个时刻我是 Leader，并且我已提交到第 N 条」，然后 Follower 至少等到本地应用到第 <code>commitIndex</code> 条，才允许读。细节见 <a href=\"https://raft.github.io/raft.pdf\">Raft 论文</a>，也可以回看<a href=\"../Text/ch1.xhtml\">第一章</a>里那次跨机房延迟。",
            "难点在于「等」这件事。等得太粗，延迟会<i>抖</i>；等得太细，唤醒的开销会吃掉收益。",
        ],
    ),
    (
        "第三章　把通知当成条件变量是常见的错误",
        [
            "很多实现里，apply 协程会在提交成功后广播一个信号，所有等待读的协程被唤醒，然后重新比较自己的水位。这在语义上是「通知」，不是「条件变量」——通知是有损的，唤醒不等于条件成立。",
            "!<blockquote>唤醒只是提示，条件仍然要自己再验一遍；把通知当条件变量用，跨机房的抖动就会一路放大。</blockquote>",
            "当批量提交把 512 条日志当成一次提交时，这个损失会被放大：等待方被唤醒，发现还差三条，只能再等下一批。跨机房一次往返 30ms，抖动就是这么来的。",
            "修法有两种。一种是缩小批大小，代价是写吞吐下降；另一种是给每个等待方挂一个按索引排序的最小堆，apply 完成后只唤醒真正满足条件的那几个。核心循环大概长这样：",
            "!<pre>for _, w := range waiters {\n    if w.index &lt;= applied {\n        w.ch &lt;- struct{}{}\n    }\n}</pre>",
            "具体两种改法的取舍：",
            "!<ul><li>缩小批大小：实现最简单，写吞吐掉 15% 左右</li><li>按索引挂最小堆：多点代码，只唤醒真正满足条件的等待方</li></ul>",
            "我们选了后者。压测下 CPU 多用了 2%，P99 从 60ms 回到 11ms。这个交换是划算的。",
        ],
    ),
    (
        "第四章　一次完整的排查记录",
        [
            "这一章按时间顺序记一次线上问题，用来验证长文分页——真实的排查记录本来就是流水账，段落多、单段短。",
            "14:02 值班群里有人说，订单详情页偶发超时，比例大概千分之三，刷新一次通常就好了。",
            "14:05 先看监控。网关 P99 从 120ms 抬到 900ms，后端服务本身没有报错，错误率是平的。",
            "14:07 排除发布。当天没有上线，最近一次变更在三天前，且只改了一个日志字段。",
            "14:12 排除资源。四台机器 CPU 都在 30% 以下，内存没有接近上限，GC 次数正常。",
            "14:18 开始怀疑下游。订单详情要串行查三个服务：商品、库存、优惠。逐个打点看耗时分布。",
            "14:21 商品服务稳定在 8ms，库存服务稳定在 12ms，优惠服务的 P99 从 20ms 跳到了 780ms。",
            "14:24 优惠服务的人说，他们最近加了一个远程配置拉取，放在请求路径里，超时设的是 800ms。",
            "14:26 这就是全部答案：远程配置中心抖动，每个请求都要等它超时才继续，超时值等于把延迟直接钉在 800ms。",
            "14:29 临时处置：把配置缓存到本地，每 30 秒异步刷新一次，请求路径不再同步拉取。延迟回到 25ms。",
            "14:40 复盘第一条：任何放在请求路径上的远程调用，都必须有低延迟的兜底值，不能拿超时值当默认值。",
            "14:42 复盘第二条：这个依赖没有出现在任何架构图上，也没有被纳入容量评估。",
            "14:45 复盘第三条：千分之三的比例骗了我们。看起来像偶发，其实是配置中心每 30 秒重连一次的窗口期。",
            "15:00 开始补监控。给每个跨服务调用加上「耗时超过基线 3 倍」的告警，而不是只看错误率。",
            "15:20 顺手把一个隐藏问题也翻了上来：优惠服务的客户端连接池上限是 8，而实例数是 16。",
            "15:24 也就是说，有一半的实例在排队等连接。平时压力小看不出来，一旦上游变慢就会雪崩。",
            "15:30 连接池调到 64，加上获取连接超时 200ms。这条改动比配置缓存更关键。",
            "16:00 重放前一天的流量做验证。P99 稳定在 130ms，没有再出现锯齿。",
            "16:10 记录一个容易被忽略的细节：这次问题的根因是「超时值当作默认值」，不是配置中心本身。",
            "16:15 如果配置中心一直很快，这个设计可以撑很久；一旦它慢下来，延迟就会原地翻六倍。",
            "16:30 把这条写进代码评审清单：请求路径上不允许出现没有兜底的远程调用。",
            "16:45 评审清单新增第二条：任何新增依赖必须同时更新架构图与容量评估，否则不予合入。",
            "17:00 最后统计影响面：受影响请求约 1.2 万次，没有产生资损，但触发了 37 条客诉。",
            "17:10 客诉的处置比技术修复更麻烦，这部分不属于本文范围，略过。",
            "17:30 收尾。把今天的排查路径补进值班手册，下次遇到同类问题可以直接跳到第 6 步。",
            "17:40 手册里写清楚一件事：先看下游耗时分布，再看自己的代码，顺序反了会浪费半小时。",
        ],
    ),
]


def xhtml(title, paras):
    # 以 ! 开头的条目是原样插入的块（blockquote / pre / ul），其余包成 <p>
    parts = []
    for p in paras:
        parts.append(p[1:] if p.startswith("!") else f"<p>{p}</p>")
    body = "\n    ".join(parts)
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

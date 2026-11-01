"""把「给同学讲的架构说明.md」转成单文件 HTML（内嵌两张 SVG 图解）。

不带任何外部依赖：字体走系统栈，图片是内联 SVG，直接双击就能看、也能发给别人。

用法：python scripts/build_doc_html.py
产物：docs/给同学讲的架构说明.html
"""

from __future__ import annotations

import html
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "docs" / "给同学讲的架构说明.md"
OUT = ROOT / "docs" / "给同学讲的架构说明.html"

# ── 两张图解（内联进 HTML，不引用外部文件）─────────────────────────
SVG_ARCH = """<figure class="fig">
<svg viewBox="0 0 680 500" width="100%" role="img" xmlns="http://www.w3.org/2000/svg">
<title>PurePhotoExplorer 三层架构</title>
<desc>界面层用 React，通过 92 个命令访问 Rust 后端，后端再读写数据库、缓存与原图。</desc>
<defs>
<marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
<path d="M2 1L8 5L2 9" fill="none" stroke="context-stroke" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
</marker>
</defs>
<rect x="40" y="40" width="600" height="100" rx="12" fill="#E6F1FB" stroke="#185FA5" stroke-width="0.5"/>
<text x="62" y="66" font-size="14" font-weight="500" fill="#042C53" dominant-baseline="central">界面层 · React + TypeScript</text>
<text x="62" y="88" font-size="12" fill="#185FA5" dominant-baseline="central">23,178 行 · 只负责「点了就有反应」，不碰硬盘</text>
<rect x="62" y="102" width="104" height="24" rx="6" fill="#FFFFFF" stroke="#85B7EB" stroke-width="0.5"/>
<text x="114" y="114" font-size="12" fill="#185FA5" text-anchor="middle" dominant-baseline="central">图库网格</text>
<rect x="176" y="102" width="104" height="24" rx="6" fill="#FFFFFF" stroke="#85B7EB" stroke-width="0.5"/>
<text x="228" y="114" font-size="12" fill="#185FA5" text-anchor="middle" dominant-baseline="central">单张查看器</text>
<rect x="290" y="102" width="104" height="24" rx="6" fill="#FFFFFF" stroke="#85B7EB" stroke-width="0.5"/>
<text x="342" y="114" font-size="12" fill="#185FA5" text-anchor="middle" dominant-baseline="central">侧栏与菜单</text>
<line x1="340" y1="142" x2="340" y2="172" stroke="#888780" stroke-width="1.5" marker-end="url(#arrow)"/>
<text x="354" y="157" font-size="12" fill="#5F5E5A" dominant-baseline="central">92 个命令 · 唯一通道（沙箱里出不去）</text>
<rect x="40" y="178" width="600" height="172" rx="12" fill="#E1F5EE" stroke="#0F6E56" stroke-width="0.5"/>
<text x="62" y="204" font-size="14" font-weight="500" fill="#04342C" dominant-baseline="central">大脑层 · Rust · 13 个车间</text>
<text x="62" y="226" font-size="12" fill="#0F6E56" dominant-baseline="central">37,590 行 · 算得快、不崩、写文件出事能回滚</text>
<rect x="62" y="240" width="130" height="42" rx="8" fill="#FFFFFF" stroke="#5DCAA5" stroke-width="0.5"/>
<text x="127" y="254" font-size="13" font-weight="500" fill="#085041" text-anchor="middle" dominant-baseline="central">解码图片</text>
<text x="127" y="271" font-size="11" fill="#0F6E56" text-anchor="middle" dominant-baseline="central">pv-decode</text>
<rect x="202" y="240" width="130" height="42" rx="8" fill="#FFFFFF" stroke="#5DCAA5" stroke-width="0.5"/>
<text x="267" y="254" font-size="13" font-weight="500" fill="#085041" text-anchor="middle" dominant-baseline="central">目录卡片</text>
<text x="267" y="271" font-size="11" fill="#0F6E56" text-anchor="middle" dominant-baseline="central">pv-index</text>
<rect x="342" y="240" width="130" height="42" rx="8" fill="#FFFFFF" stroke="#5DCAA5" stroke-width="0.5"/>
<text x="407" y="254" font-size="13" font-weight="500" fill="#085041" text-anchor="middle" dominant-baseline="central">AI 打标与人脸</text>
<text x="407" y="271" font-size="11" fill="#0F6E56" text-anchor="middle" dominant-baseline="central">pv-ai</text>
<rect x="482" y="240" width="130" height="42" rx="8" fill="#FFFFFF" stroke="#5DCAA5" stroke-width="0.5"/>
<text x="547" y="254" font-size="13" font-weight="500" fill="#085041" text-anchor="middle" dominant-baseline="central">编辑写回</text>
<text x="547" y="271" font-size="11" fill="#0F6E56" text-anchor="middle" dominant-baseline="central">pv-edit</text>
<rect x="62" y="290" width="130" height="42" rx="8" fill="#FFFFFF" stroke="#5DCAA5" stroke-width="0.5"/>
<text x="127" y="304" font-size="13" font-weight="500" fill="#085041" text-anchor="middle" dominant-baseline="central">扫描目录</text>
<text x="127" y="321" font-size="11" fill="#0F6E56" text-anchor="middle" dominant-baseline="central">pv-scan</text>
<rect x="202" y="290" width="130" height="42" rx="8" fill="#FFFFFF" stroke="#5DCAA5" stroke-width="0.5"/>
<text x="267" y="304" font-size="13" font-weight="500" fill="#085041" text-anchor="middle" dominant-baseline="central">两级缓存</text>
<text x="267" y="321" font-size="11" fill="#0F6E56" text-anchor="middle" dominant-baseline="central">pv-cache</text>
<rect x="342" y="290" width="130" height="42" rx="8" fill="#FFFFFF" stroke="#5DCAA5" stroke-width="0.5"/>
<text x="407" y="304" font-size="13" font-weight="500" fill="#085041" text-anchor="middle" dominant-baseline="central">地名查询</text>
<text x="407" y="321" font-size="11" fill="#0F6E56" text-anchor="middle" dominant-baseline="central">pv-geo</text>
<rect x="482" y="290" width="130" height="42" rx="8" fill="#FFFFFF" stroke="#5DCAA5" stroke-width="0.5"/>
<text x="547" y="304" font-size="13" font-weight="500" fill="#085041" text-anchor="middle" dominant-baseline="central">EXIF 读写</text>
<text x="547" y="321" font-size="11" fill="#0F6E56" text-anchor="middle" dominant-baseline="central">pv-meta</text>
<line x1="340" y1="352" x2="340" y2="382" stroke="#888780" stroke-width="1.5" marker-end="url(#arrow)"/>
<text x="354" y="367" font-size="12" fill="#5F5E5A" dominant-baseline="central">读文件 / 写索引，全都集中在下一层</text>
<rect x="40" y="388" width="600" height="92" rx="12" fill="#FAEEDA" stroke="#854F0B" stroke-width="0.5"/>
<text x="62" y="410" font-size="14" font-weight="500" fill="#412402" dominant-baseline="central">仓库层</text>
<rect x="62" y="422" width="175" height="42" rx="8" fill="#FFFFFF" stroke="#EF9F27" stroke-width="0.5"/>
<text x="149" y="436" font-size="13" font-weight="500" fill="#633806" text-anchor="middle" dominant-baseline="central">SQLite 数据库</text>
<text x="149" y="453" font-size="11" fill="#854F0B" text-anchor="middle" dominant-baseline="central">16 张表 · 一个文件</text>
<rect x="252" y="422" width="175" height="42" rx="8" fill="#FFFFFF" stroke="#EF9F27" stroke-width="0.5"/>
<text x="339" y="436" font-size="13" font-weight="500" fill="#633806" text-anchor="middle" dominant-baseline="central">缩略图缓存</text>
<text x="339" y="453" font-size="11" fill="#854F0B" text-anchor="middle" dominant-baseline="central">按指纹命中，省掉重算</text>
<rect x="442" y="422" width="175" height="42" rx="8" fill="#FFFFFF" stroke="#EF9F27" stroke-width="0.5"/>
<text x="529" y="436" font-size="13" font-weight="500" fill="#633806" text-anchor="middle" dominant-baseline="central">你的原始照片</text>
<text x="529" y="453" font-size="11" fill="#854F0B" text-anchor="middle" dominant-baseline="central">只读，除非你主动改</text>
</svg>
<figcaption>图 1 · 三层结构：界面只管响应，后端负责算和写，数据落在最下面</figcaption>
</figure>"""

SVG_FLOW = """<figure class="fig">
<svg viewBox="0 0 680 368" width="100%" role="img" xmlns="http://www.w3.org/2000/svg">
<title>缩略图取图流程</title>
<desc>点击文件夹后，后端只按后缀列清单，界面先铺占位框，再按可见范围排队取图，命中缓存则直接返回。</desc>
<defs>
<marker id="arrow2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
<path d="M2 1L8 5L2 9" fill="none" stroke="context-stroke" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
</marker>
</defs>
<rect x="40" y="40" width="360" height="56" rx="8" fill="#E6F1FB" stroke="#185FA5" stroke-width="0.5"/>
<text x="220" y="60" font-size="13" font-weight="500" fill="#042C53" text-anchor="middle" dominant-baseline="central">你点了侧栏的「2024杭州」</text>
<text x="220" y="78" font-size="11" fill="#185FA5" text-anchor="middle" dominant-baseline="central">一次普通的单击</text>
<line x1="220" y1="96" x2="220" y2="120" stroke="#888780" stroke-width="1.5" marker-end="url(#arrow2)"/>
<rect x="40" y="124" width="360" height="56" rx="8" fill="#E6F1FB" stroke="#185FA5" stroke-width="0.5"/>
<text x="220" y="144" font-size="13" font-weight="500" fill="#042C53" text-anchor="middle" dominant-baseline="central">后端只按后缀名列清单</text>
<text x="220" y="162" font-size="11" fill="#185FA5" text-anchor="middle" dominant-baseline="central">不打开任何文件，所以快</text>
<line x1="220" y1="180" x2="220" y2="204" stroke="#888780" stroke-width="1.5" marker-end="url(#arrow2)"/>
<rect x="40" y="208" width="360" height="56" rx="8" fill="#E6F1FB" stroke="#185FA5" stroke-width="0.5"/>
<text x="220" y="228" font-size="13" font-weight="500" fill="#042C53" text-anchor="middle" dominant-baseline="central">界面先把格子铺满，里面是空的</text>
<text x="220" y="246" font-size="11" fill="#185FA5" text-anchor="middle" dominant-baseline="central">先给形状，再填内容</text>
<line x1="220" y1="264" x2="220" y2="288" stroke="#888780" stroke-width="1.5" marker-end="url(#arrow2)"/>
<rect x="40" y="292" width="360" height="56" rx="8" fill="#E1F5EE" stroke="#0F6E56" stroke-width="0.5"/>
<text x="220" y="312" font-size="13" font-weight="500" fill="#04342C" text-anchor="middle" dominant-baseline="central">按「看得见」排队取图，最多同时 4 个</text>
<text x="220" y="330" font-size="11" fill="#0F6E56" text-anchor="middle" dominant-baseline="central">缓存命中直接回，没命中才解码</text>
<line x1="400" y1="68" x2="430" y2="68" stroke="#B4B2A9" stroke-width="0.5" stroke-dasharray="3 3"/>
<text x="436" y="68" font-size="12" fill="#5F5E5A" dominant-baseline="central">一切从这次点击开始</text>
<line x1="400" y1="152" x2="430" y2="152" stroke="#B4B2A9" stroke-width="0.5" stroke-dasharray="3 3"/>
<text x="436" y="152" font-size="12" fill="#5F5E5A" dominant-baseline="central">约 30 毫秒</text>
<line x1="400" y1="236" x2="430" y2="236" stroke="#B4B2A9" stroke-width="0.5" stroke-dasharray="3 3"/>
<text x="436" y="236" font-size="12" fill="#5F5E5A" dominant-baseline="central">你已经觉得「加载好了」</text>
<line x1="400" y1="320" x2="430" y2="320" stroke="#B4B2A9" stroke-width="0.5" stroke-dasharray="3 3"/>
<text x="436" y="320" font-size="12" fill="#5F5E5A" dominant-baseline="central">只解 1/64 的像素</text>
</svg>
<figcaption>图 2 · 从点一下到看见缩略图：快的原因不是算得快，是「先摆空框、只解能看见的」</figcaption>
</figure>"""


# ── markdown 解析（只覆盖本文用到的语法）───────────────────────────
INLINE_CODE_RE = re.compile(r"`([^`]+)`")


def inline(text: str) -> str:
    """行内标记：`code`、**粗体**。先摘出 code，避免它的符号被当成强调。"""
    stash: list[str] = []

    def hold(m: re.Match[str]) -> str:
        stash.append(m.group(1))
        return f"\x00{len(stash) - 1}\x00"

    text = INLINE_CODE_RE.sub(hold, text)
    text = html.escape(text, quote=False)
    text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
    text = re.sub(
        r"\x00(\d+)\x00",
        lambda m: f"<code>{html.escape(stash[int(m.group(1))], quote=False)}</code>",
        text,
    )
    return text


def is_table_sep(line: str) -> bool:
    s = line.strip()
    return bool(s) and s.startswith("|") and set(s) <= set("|-: ")


def parse_table(lines: list[str]) -> str:
    cells_head = [c.strip() for c in lines[0].strip().strip("|").split("|")]
    rows = []
    for line in lines[2:]:
        rows.append([c.strip() for c in line.strip().strip("|").split("|")])
    out = ['<div class="table-wrap"><table><thead><tr>']
    for c in cells_head:
        out.append(f"<th>{inline(c)}</th>")
    out.append("</tr></thead><tbody>")
    for r in rows:
        out.append("<tr>")
        for c in r:
            out.append(f"<td>{inline(c)}</td>")
        out.append("</tr>")
    out.append("</tbody></table></div>")
    return "".join(out)


def convert(md: str) -> tuple[str, list[tuple[int, str, str]]]:
    """返回 (正文 HTML, [(级别, id, 文本)])。"""
    lines = md.split("\n")
    out: list[str] = []
    toc: list[tuple[int, str, str]] = []

    i = 0
    n = len(lines)
    cur_h2 = ""          # 当前所处的一级章节标题（用来决定图插在哪）
    arch_done = False    # 第 1 节的 ASCII 框是否已换成图 1
    flow_done = False    # 图 2 是否已插过
    para_in_h2 = 0       # 当前章节已输出的段落数

    while i < n:
        line = lines[i]

        # 空行
        if not line.strip():
            i += 1
            continue

        # 分隔线
        if line.strip() == "---":
            out.append('<hr class="sep"/>')
            i += 1
            continue

        # 代码块
        if line.startswith("```"):
            lang = line[3:].strip()
            i += 1
            body: list[str] = []
            while i < n and not lines[i].startswith("```"):
                body.append(lines[i])
                i += 1
            i += 1
            code = html.escape("\n".join(body), quote=False)
            if not arch_done and "整体结构" in cur_h2:
                # 第 1 节那张 ASCII 架构框 → 换成矢量图（浏览器里框线对不齐）
                out.append(SVG_ARCH)
                arch_done = True
                continue
            cls = f' class="lang-{lang}"' if lang else ""
            out.append(f'<pre class="code"><code{cls}>{code}</code></pre>')
            continue

        # 标题
        m = re.match(r"^(#{1,4})\s+(.*)$", line)
        if m:
            level = len(m.group(1))
            text = m.group(2).strip()
            hid = f"h{len(toc) + 1}"
            toc.append((level, hid, text))
            if level == 1:
                # h1 已经在 hero 里放过了
                i += 1
                continue
            if level == 2:
                cur_h2 = text
                para_in_h2 = 0
            out.append(f'<h{level} id="{hid}">{inline(text)}</h{level}>')
            i += 1
            continue

        # 引用块
        if line.startswith(">"):
            buf: list[str] = []
            while i < n and lines[i].startswith(">"):
                buf.append(lines[i].lstrip(">").strip())
                i += 1
            # 空行分段
            chunks = "\n".join(buf).split("\n\n")
            out.append('<blockquote class="note">')
            for ch in chunks:
                seg = " ".join(x for x in ch.split("\n") if x).strip()
                if seg:
                    out.append(f"<p>{inline(seg)}</p>")
            out.append("</blockquote>")
            continue

        # 表格
        if line.strip().startswith("|") and i + 1 < n and is_table_sep(lines[i + 1]):
            tbl: list[str] = []
            while i < n and lines[i].strip().startswith("|"):
                tbl.append(lines[i])
                i += 1
            out.append(parse_table(tbl))
            continue

        # 无序列表
        if re.match(r"^-\s+", line):
            out.append("<ul>")
            while i < n and re.match(r"^-\s+", lines[i]):
                out.append(f"<li>{inline(lines[i][2:].strip())}</li>")
                i += 1
            out.append("</ul>")
            continue

        # 有序列表
        if re.match(r"^\d+\.\s+", line):
            out.append("<ol>")
            while i < n and re.match(r"^\d+\.\s+", lines[i]):
                txt = re.sub(r"^\d+\.\s+", "", lines[i]).strip()
                out.append(f"<li>{inline(txt)}</li>")
                i += 1
            out.append("</ol>")
            continue

        # 段落
        buf = [line.strip()]
        i += 1
        while i < n:
            nxt = lines[i]
            if (
                not nxt.strip()
                or nxt.startswith("#")
                or nxt.startswith(">")
                or nxt.startswith("```")
                or nxt.strip() == "---"
                or nxt.strip().startswith("|")
                or re.match(r"^-\s+", nxt)
                or re.match(r"^\d+\.\s+", nxt)
            ):
                break
            buf.append(nxt.strip())
            i += 1
        out.append(f"<p>{inline(''.join(buf))}</p>")
        para_in_h2 += 1
        # 图 2 插在第 4 节导语之后（先给图，再看"假设你点了…"）
        if not flow_done and "完整旅程" in cur_h2 and para_in_h2 == 1:
            out.append(SVG_FLOW)
            flow_done = True

    return "\n".join(out), toc


CSS = """
:root{
  --font-sans:"PingFang SC","Microsoft YaHei UI","Microsoft YaHei","Segoe UI",system-ui,sans-serif;
  --font-mono:"Cascadia Code","Cascadia Mono",Consolas,"SF Mono",Menlo,monospace;
  --bg:#FBFAF7;
  --surface:#FFFFFF;
  --text:#26262A;
  --muted:#63625D;
  --faint:#8A8983;
  --line:#E6E3DA;
  --accent:#185FA5;
  --accent-soft:#E6F1FB;
  --warm:#854F0B;
  --warm-soft:#FAEEDA;
  --code-bg:#F5F4EE;
}
*{box-sizing:border-box}
html{-webkit-text-size-adjust:100%}
body{
  margin:0;background:var(--bg);color:var(--text);
  font-family:var(--font-sans);font-size:16px;line-height:1.85;
  -webkit-font-smoothing:antialiased;
}
.wrap{max-width:1180px;margin:0 auto;padding:0 28px}

.hero{padding:72px 0 44px;border-bottom:1px solid var(--line);background:var(--surface)}
.kicker{margin:0 0 14px;font-size:13px;letter-spacing:.12em;color:var(--faint);text-transform:uppercase}
.hero h1{margin:0 0 20px;font-size:38px;line-height:1.3;font-weight:500;letter-spacing:-.01em}
.lede{margin:0;max-width:660px;font-size:17px;color:var(--muted)}
.facts{display:flex;flex-wrap:wrap;gap:10px;margin:28px 0 0;padding:0;list-style:none}
.facts li{
  font-size:13px;color:var(--muted);background:var(--bg);
  border:1px solid var(--line);border-radius:999px;padding:5px 14px;
}

.layout{display:grid;grid-template-columns:268px minmax(0,1fr);gap:52px;padding:48px 0 96px}
.toc{position:sticky;top:28px;align-self:start;max-height:calc(100vh - 56px);overflow:auto}
.toc h2{margin:0 0 14px;font-size:12px;letter-spacing:.08em;color:var(--faint);font-weight:500;text-transform:uppercase}
.toc ol{margin:0;padding:0;list-style:none;border-left:1px solid var(--line)}
.toc li.grp{position:relative}
.toc li.grp>a{
  display:block;
  padding:5px 16px 5px 14px;margin-left:-1px;border-left:2px solid transparent;
  color:var(--muted);text-decoration:none;font-size:13px;line-height:1.55;cursor:pointer;
}
.toc li.grp>a:hover{color:var(--accent)}
.toc li.grp>a.lv3{padding-left:28px;font-size:12.5px;color:var(--faint)}
.toc a.on{color:var(--accent);border-left-color:var(--accent);font-weight:500}
.toc .caret{position:absolute;right:2px;top:9px;font-size:9px;color:var(--faint);transition:transform .15s ease}
.toc li.grp.open>a .caret{transform:rotate(90deg)}
.toc .sub{display:none;margin:0;padding:0;list-style:none;border-left:0}
.toc li.grp.open .sub{display:block}
.toc .sub a{
  display:block;padding:4px 0 4px 28px;margin-left:-1px;border-left:2px solid transparent;
  color:var(--faint);text-decoration:none;font-size:12.5px;line-height:1.5;
}
.toc .sub a:hover{color:var(--accent)}
.toc .sub a.on{color:var(--accent);border-left-color:var(--accent);font-weight:500}

.doc{min-width:0;max-width:760px}
.doc h2{
  margin:64px 0 20px;font-size:26px;line-height:1.35;font-weight:500;
  padding-bottom:12px;border-bottom:1px solid var(--line);
}
.doc h2:first-child{margin-top:0}
.doc h3{margin:40px 0 14px;font-size:19px;font-weight:500;color:var(--text)}
.doc p{margin:0 0 18px}
.doc strong{font-weight:500;color:#111114}
.doc ul,.doc ol{margin:0 0 20px;padding-left:22px}
.doc li{margin:7px 0}
.doc li::marker{color:var(--faint)}

.doc code{
  font-family:var(--font-mono);font-size:.875em;
  background:var(--code-bg);border:1px solid var(--line);border-radius:4px;
  padding:1px 5px;color:#3C3489;
}
pre.code{
  margin:0 0 22px;padding:18px 20px;background:var(--code-bg);
  border:1px solid var(--line);border-radius:10px;overflow-x:auto;
  font-family:var(--font-mono);font-size:13px;line-height:1.7;
}
pre.code code{background:none;border:0;padding:0;color:#33323A;font-size:13px;white-space:pre}

.table-wrap{margin:0 0 24px;overflow-x:auto;border:1px solid var(--line);border-radius:10px;background:var(--surface)}
table{border-collapse:collapse;width:100%;font-size:14.5px}
th,td{padding:11px 16px;text-align:left;vertical-align:top;border-bottom:1px solid var(--line)}
th{background:var(--bg);font-weight:500;color:var(--muted);font-size:13px;white-space:nowrap}
tbody tr:last-child td{border-bottom:0}
tbody tr:hover{background:#FCFBF7}
td code{font-size:13px}
td:first-child code{white-space:nowrap}

blockquote.note{
  margin:0 0 22px;padding:16px 20px;background:var(--accent-soft);
  border-left:3px solid var(--accent);border-radius:0 8px 8px 0;color:#1B3A57;
}
blockquote.note p{margin:0 0 10px;font-size:15px}
blockquote.note p:last-child{margin:0}

hr.sep{border:0;border-top:1px solid var(--line);margin:48px 0}

figure.fig{margin:8px 0 28px;padding:20px 16px 12px;background:var(--surface);
  border:1px solid var(--line);border-radius:12px}
figure.fig svg{display:block;width:100%;height:auto}
figcaption{margin-top:12px;text-align:center;font-size:13px;color:var(--faint)}

footer.end{border-top:1px solid var(--line);padding:32px 0 64px;background:var(--surface)}
footer.end p{margin:0;font-size:13.5px;color:var(--faint)}

@media (max-width:960px){
  .wrap{padding:0 20px}
  .hero{padding:48px 0 32px}
  .hero h1{font-size:29px}
  .layout{grid-template-columns:minmax(0,1fr);gap:32px;padding:32px 0 64px}
  .toc{position:static;max-height:none;padding-bottom:8px;border-bottom:1px solid var(--line)}
  .toc ol{columns:2;column-gap:24px;border-left:0}
  .toc a{padding-left:0}
  .toc a.lv3{display:none}
  .doc h2{font-size:22px;margin-top:48px}
}
@media print{
  body{background:#fff;font-size:11.5pt}
  .toc{display:none}
  .layout{display:block;padding:0}
  .hero{border:0;padding:0 0 24px}
  .doc{max-width:none}
  pre.code,figure.fig,.table-wrap{break-inside:avoid}
  .doc h2{break-after:avoid}
}
"""

JS = """
(function(){
  var groups = Array.prototype.slice.call(document.querySelectorAll('.toc li.grp'));
  var links = Array.prototype.slice.call(document.querySelectorAll('.toc a[href^="#"]'));

  groups.forEach(function(li){
    var a = li.querySelector('a.lv2');
    if(!a || !li.querySelector('.sub')) return;
    a.addEventListener('click', function(){
      groups.forEach(function(o){ if(o !== li) o.classList.remove('open'); });
      li.classList.toggle('open');
    });
  });

  var targets = links.map(function(a){
    return document.getElementById(a.getAttribute('href').slice(1));
  }).filter(Boolean);
  if(!targets.length) return;

  var io = new IntersectionObserver(function(entries){
    entries.forEach(function(e){
      if(!e.isIntersecting) return;
      var id = e.target.id;
      links.forEach(function(a){
        a.classList.toggle('on', a.getAttribute('href') === '#' + id);
      });
      var cur = document.getElementById(id);
      while(cur && cur !== document.body){
        if(cur.tagName === 'H2'){
          groups.forEach(function(o){
            var a = o.querySelector('a.lv2');
            if(a && a.getAttribute('href') === '#' + cur.id) o.classList.add('open');
          });
          break;
        }
        cur = cur.parentElement;
      }
    });
  }, {rootMargin:'-8% 0px -75% 0px', threshold:0});
  targets.forEach(function(t){ io.observe(t); });
})();
"""


def extract_hero(md: str) -> tuple[str, str]:
    """从 md 里取 h1 标题与导语（h1 之后第一段，引用块也算）。"""
    lines = md.split("\n")
    title = ""
    start = 0
    for idx, ln in enumerate(lines):
        if ln.startswith("# "):
            title = ln[2:].strip()
            start = idx + 1
            break
    i = start
    while i < len(lines) and not lines[i].strip():
        i += 1
    buf: list[str] = []
    while i < len(lines):
        s = lines[i].strip()
        if not s or s.startswith(("#", "---", "```", "|")):
            break
        buf.append(s.lstrip("> ").strip())
        i += 1
    lede = re.sub(r"\*\*|`", "", " ".join(buf))
    if len(lede) > 58:
        lede = lede[:58].rstrip("，。、") + "…"
    return title, lede


def build(src: Path | None = None, facts: str = "", out: Path | None = None, lede_override: str = "") -> None:
    src = src or SRC
    out = out or src.with_suffix(".html")
    md = src.read_text(encoding="utf-8")
    body, toc = convert(md)
    title, lede = extract_hero(md)
    if lede_override:
        lede = lede_override

    # 目录：按 h2 分组，h3 收进各组（默认折叠，点开才展开）
    groups: list[tuple[str, str, list[tuple[str, str]]]] = []
    for level, hid, text in toc:
        if level == 1:
            continue
        if level == 2:
            groups.append((text, hid, []))
        elif level == 3 and groups:
            groups[-1][2].append((text, hid))

    toc_items: list[str] = []
    for text, hid, subs in groups:
        sub_html = ""
        if subs:
            inner = "".join(
                f'<li><a href="#{sid}">{html.escape(stext)}</a></li>' for stext, sid in subs
            )
            sub_html = f'<ul class="sub">{inner}</ul>'
        caret = '<span class="caret">▶</span>' if subs else ""
        toc_items.append(
            f'<li class="grp"><a class="lv2" href="#{hid}">'
            f'<span>{html.escape(text)}</span>{caret}</a>{sub_html}</li>'
        )

    fact_items = "".join(f"<li>{html.escape(f.strip())}</li>" for f in facts.split("|") if f.strip())

    doc = f"""<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>{html.escape(title)} · PurePhotoExplorer</title>
<meta name="description" content="{html.escape(lede)}"/>
<style>{CSS}</style>
</head>
<body>
<header class="hero">
  <div class="wrap">
    <p class="kicker">PurePhotoExplorer · 技术说明</p>
    <h1>{html.escape(title)}</h1>
    <p class="lede">{html.escape(lede)}</p>
    <ul class="facts">{fact_items}</ul>
  </div>
</header>
<div class="wrap">
  <div class="layout">
    <nav class="toc"><h2>目录</h2><ol>{''.join(toc_items)}</ol></nav>
    <main class="doc">
{body}
    </main>
  </div>
</div>
<footer class="end"><div class="wrap"><p>基于 2026-11-01 的代码状态生成 · 行数为生产代码，不含测试模块</p></div></footer>
<script>{JS}</script>
</body>
</html>
"""
    out.write_text(doc, encoding="utf-8")
    try:
        shown = out.resolve().relative_to(ROOT)
    except ValueError:
        shown = out
    print(f"写出 {shown} · {len(doc):,} 字符")


if __name__ == "__main__":
    import sys

    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    fact_arg = ""
    lede_arg = ""
    for a in sys.argv[1:]:
        if a.startswith("--facts="):
            fact_arg = a.split("=", 1)[1]
        elif a.startswith("--lede="):
            lede_arg = a.split("=", 1)[1]
    src_path = Path(args[0]) if args else None
    build(src_path, fact_arg, None, lede_arg)

/**
 * 图库模式的帮助浮层（顶栏「设置」右边的「帮助」）。
 *
 * ## 内容顺序（用户 2026-11-01 定）
 * **这是啥 → 能干什么 → 怎么干**，不要一上来就讲快捷键。
 * 第一次点「帮助」的人通常还没搞清"这软件和资源管理器有什么区别"，
 * 直接甩操作说明等于默认他已经懂了。
 *
 * 1. 软件简介：一句话定位 + 一句"数据不出本机"（这是和网盘类产品的核心差别）；
 * 2. 能做什么：按用途分组，一行一类。**只写界面上真有的** ——
 *    加密图库（`pv-vault`）后端已实现但前端没有入口，就不写，
 *    否则用户翻遍界面找不到，会以为是自己不会用；
 * 3. 怎么用：三个区域 → 「看一眼 vs 入库」→ 鼠标语义；
 * 4. 快捷键放在最后：它是查阅项，不是入门项。
 *
 * ⛔ 别在文案里写「即将支持」：帮助里出现未实现的东西，用户只会怀疑自己。
 */

import { ModalShell } from '../viewer/ModalShell'

interface ManagerHelpProps {
  onClose: () => void
}

/** 一根说明行：左列文字标签，右列解释。 */
function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="pv-modal-row">
      <dt className="pv-about-label">{label}</dt>
      <dd>{children}</dd>
    </div>
  )
}

/** 一根快捷键行：左列按键胶囊。 */
function KeyRow({ keys, children }: { keys: string; children: React.ReactNode }) {
  return (
    <div className="pv-modal-row">
      <dt>
        <kbd className="pv-kbd">{keys}</kbd>
      </dt>
      <dd>{children}</dd>
    </div>
  )
}

export function ManagerHelp({ onClose }: ManagerHelpProps) {
  return (
    <ModalShell title="帮助" onClose={onClose} width="wide">
      <section className="pv-modal-section">
        <h3 className="pv-modal-group">这个软件是做什么的</h3>
        <p className="pv-help-lede">
          PurePhotoExplorer 是一个装在 Windows 上的<strong>本地照片管理工具</strong>。
          照片散在几个盘、几十个文件夹里也没关系，它负责登记成册，之后按时间、地点、标签、相机
          把它们找回来。所有解码、识别都在你自己的电脑上完成 —— 不联网，也不上传照片。
        </p>
      </section>

      <section className="pv-modal-section">
        <h3 className="pv-modal-group">能做什么</h3>
        <dl className="pv-modal-list">
          <Row label="看图">
            JPEG、PNG、GIF、WebP、HEIC、BMP、TIFF、AVIF、PSD，以及 8 种相机 RAW。动图会动。
          </Row>
          <Row label="整理">
            建多个图库；把文件夹加进图库；智能图库（今年 / 本月 / 四星以上 / 只看 RAW）。
          </Row>
          <Row label="找照片">
            搜文件名、标签、地点、机身与镜头；按格式、年份、相机、地点筛。多个条件可以叠加。
          </Row>
          <Row label="打标签">
            自己打，也可以让 AI 自动认出照片里有什么（风景、猫、美食……），全都本地跑。
          </Row>
          <Row label="认人脸">
            自动把同一个人的照片归到一起，可以给他起名字，之后点名字就能筛出来。
          </Row>
          <Row label="编辑">
            旋转、翻转、裁剪、调亮度对比度；另存为别的格式；批量转格式、按 EXIF 摆正。
          </Row>
          <Row label="输出">
            导出（复制原图）、打印（先看版面再送印）、批量重命名、复制或移动到文件夹。
          </Row>
          <Row label="其它">
            直接看压缩包（zip / 7z / rar / cbz）里的图；放映幻灯片；设为图片的默认打开方式。
          </Row>
        </dl>
      </section>

      <section className="pv-modal-section">
        <h3 className="pv-modal-group">第一次用，按这个顺序</h3>
        <dl className="pv-modal-list">
          <Row label="① 找到文件夹">左边展开「我的电脑」，一路点到自己放照片的文件夹。</Row>
          <Row label="② 右键 → 加入图库">
            连子文件夹一起加也行。底部会显示进度，加完之前先别关窗口。
          </Row>
          <Row label="③ 点左边的图库名">
            照片就在中间了，可以搜、可以筛。想让电脑自动认出拍的是什么，点「AI 识别」。
          </Row>
        </dl>
      </section>

      <section className="pv-modal-section">
        <h3 className="pv-modal-group">窗口怎么用</h3>
        <dl className="pv-modal-list">
          <Row label="左侧栏">上面是「我的电脑」，下面是图库。点一下只是切换到哪里看。</Row>
          <Row label="中间">照片。右上角切成列表，右下角调缩略图大小。</Row>
          <Row label="底部一行">当前范围、命中多少张、选中了什么。出错也显示在这里。</Row>
          <Row label="单击 / 双击">单击加选，双击打开看大图（文件夹则是进去）。</Row>
          <Row label="再点一次已选中的">取消这一次选中的全部，不是只取消这一张。</Row>
          <Row label="点空白处">全部取消。</Row>
          <Row label="右键卡片 / 空白处">对着照片 = 只操作这几张；对着空白 = 操作整个范围（刷新、全选）。</Row>
        </dl>
      </section>

      <section className="pv-modal-section">
        <h3 className="pv-modal-group">「看一眼」和「入库」不是一回事</h3>
        <dl className="pv-modal-list">
          <Row label="左键点文件夹">
            只是<strong>预览</strong>：不写索引、不生成缓存。所以搜不到、也打不了标签。
          </Row>
          <Row label="右键 → 加入图库">
            才是<strong>真的登记</strong>：之后能搜索、按标签筛、按地点找、跑 AI 识别人像。
          </Row>
          <Row label="更快的方式">把文件夹直接拖到左侧某个图库名字上。</Row>
        </dl>
      </section>

      <section className="pv-modal-section">
        <h3 className="pv-modal-group">快捷键</h3>
        <dl className="pv-modal-list">
          <KeyRow keys="Ctrl + O">打开一个文件夹</KeyRow>
          <KeyRow keys="Ctrl + A">全选当前看到的结果</KeyRow>
          <KeyRow keys="Ctrl + P">打印（先看版面再送印）</KeyRow>
          <KeyRow keys="Delete">删除选中的照片（进回收站，不会直接抹掉）</KeyRow>
          <KeyRow keys="Ctrl + ,">打开设置</KeyRow>
          <KeyRow keys="Esc">关掉这个窗口 / 回到看图</KeyRow>
        </dl>
      </section>
    </ModalShell>
  )
}

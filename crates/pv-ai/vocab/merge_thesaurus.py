#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""把《照片库中文主题词表》(518 条) 优化成 500 条「类目名口语化 + 按模型能力取舍」的定版。

## 这一版改了两件事（2026-10-20 晚）
1. **一级/二级类目改成日常词**（原来「事件与生活时刻」「自然与风光」「屏幕与文档」这种
   书面腔不适合当筛选器）：人物 / 活动 / 旅行 / 风景 / 天气 / 动物 / 植物 / 城市 / 建筑 /
   室内 / 美食 / 物品 / 交通 / 运动 / 文化 / 行业 / 风格 / 材质 / 文档 —— 19 个。
   二级同理：花卉→花、树木→树、鸟类→鸟、家居用品→家居、机动车→汽车、轨道交通→轨道……
2. **补齐证件类**：证件 / 身份证 / 护照 / 驾驶证 / 毕业证 / 证书 / 名片 / 银行卡 /
   发票 / 合同 / 账单 / 机票（原表只有「证件照」，那是照片类型，不是证件本身）。

## 按模型能力的取舍（上一版已定，本版沿用）
- 删抽象情感（CLIP 伪相关）、删时间与季节（EXIF 能算）、删题材摄影（与内容重复）、
  删无视觉定义的行业词、删会误命中的日期类节日。
- 补 屏幕与文档 / 二次元等风格词 / 键盘显示器无人机 / 奶茶便当机场。

## 产物
- `zh_tags.csv`：500 条，`编码,一级,二级,主题词,同义词,英文`
- `词表预览.html`：搜中文/同义词/英文 + 一级类目筛选
"""

import collections
import csv
import html
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
# 源表已内嵌到仓库（原稿 `...\\photo_thesaurus\\照片库中文主题词表.csv`），
# 换机器/换目录也能重新生成，不依赖外部路径。
SRC = HERE / "source_518.csv"

# ── 一级类目展示顺序（越靠前越常用）──────────────────────────────
L1_ORDER = ["人物", "活动", "旅行", "风景", "天气", "动物", "植物", "城市", "建筑",
            "室内", "美食", "物品", "交通", "军事", "运动", "文化", "行业", "风格", "材质", "文档"]

# ── 二级映射：(原一级, 原二级) → (新一级, 新二级) ──────────────────
SUB_MAP = {
    ("人物与人像", "身份与关系"): ("人物", "人群"),
    ("人物与人像", "人像类型"): ("人物", "人像"),
    ("人物与人像", "状态与动作"): ("人物", "表情动作"),
    ("人物与人像", "职业人物"): ("人物", "职业"),

    ("事件与生活时刻", "人生仪礼"): ("活动", "人生大事"),
    ("事件与生活时刻", "社交活动"): ("活动", "聚会"),
    ("事件与生活时刻", "公共事件"): ("活动", "现场"),
    ("事件与生活时刻", "传统节日"): ("活动", "中国节日"),
    ("事件与生活时刻", "现代节日"): ("活动", "外国节日"),

    ("旅行与户外", "旅行方式"): ("旅行", "出行方式"),
    ("旅行与户外", "住宿设施"): ("旅行", "住宿"),
    ("旅行与户外", "目的地类型"): ("旅行", "目的地"),
    ("旅行与户外", "户外活动"): ("旅行", "玩法"),

    ("自然与风光", "地貌"): ("风景", "地貌"),
    ("自然与风光", "水域"): ("风景", "水景"),
    ("自然与风光", "天象景观"): ("风景", "天象"),
    ("自然与风光", "田园景观"): ("风景", "田园"),

    ("天气与天空", "天气现象"): ("天气", "天气"),
    ("天气与天空", "天空景象"): ("天气", "天空"),

    ("动物", "宠物"): ("动物", "宠物"),
    ("动物", "家禽家畜"): ("动物", "家畜"),
    ("动物", "鸟类"): ("动物", "鸟"),
    ("动物", "野生动物"): ("动物", "野生"),
    ("动物", "水生动物"): ("动物", "水生"),
    ("动物", "两栖爬行与昆虫"): ("动物", "昆虫"),

    ("植物与花卉", "花卉"): ("植物", "花"),
    ("植物与花卉", "树木"): ("植物", "树"),
    ("植物与花卉", "绿植与园艺"): ("植物", "绿植"),

    ("城市与建筑", "城市景观"): ("城市", "街景"),
    ("城市与建筑", "公共设施"): ("城市", "公共设施"),
    ("室内空间", "居家空间"): ("室内", "家里"),
    ("室内空间", "公共室内"): ("室内", "公共场所"),

    ("美食与饮品", "中式美食"): ("美食", "中餐"),
    ("美食与饮品", "甜品"): ("美食", "甜点"),
    ("美食与饮品", "异国料理"): ("美食", "西餐"),
    ("美食与饮品", "食材与水果"): ("美食", "食材"),
    ("美食与饮品", "餐饮场景"): ("美食", "用餐"),
    ("美食与饮品", "饮品"): ("美食", "饮品"),

    ("物品与静物", "服装"): ("物品", "衣服"),
    ("物品与静物", "配饰"): ("物品", "配饰"),
    ("物品与静物", "家居用品"): ("物品", "家居"),
    ("物品与静物", "文化与日用品"): ("物品", "杂项"),
    ("物品与静物", "数码产品"): ("物品", "数码"),

    ("交通与载具", "机动车"): ("交通", "汽车"),
    ("交通与载具", "非机动车"): ("交通", "两轮"),
    ("交通与载具", "轨道交通"): ("交通", "轨道"),
    ("交通与载具", "航空器"): ("交通", "飞机"),
    ("交通与载具", "船舶"): ("交通", "船"),
    ("交通与载具", "交通枢纽"): ("交通", "枢纽"),

    ("体育与健身", "球类运动"): ("运动", "球类"),
    ("体育与健身", "水上运动"): ("运动", "水上"),
    ("体育与健身", "田径与耐力"): ("运动", "跑步"),
    ("体育与健身", "健身与室内"): ("运动", "健身"),
    ("体育与健身", "搏击与技击"): ("运动", "搏击"),
    ("体育与健身", "极限与户外"): ("运动", "极限"),

    ("文化艺术与娱乐", "视觉艺术"): ("文化", "美术"),
    ("文化艺术与娱乐", "表演艺术"): ("文化", "表演"),
    ("文化艺术与娱乐", "休闲爱好"): ("文化", "爱好"),

    ("行业与生产", "农业"): ("行业", "农业"),
    ("行业与生产", "工业与工程"): ("行业", "工业"),
    ("行业与生产", "商业与服务"): ("行业", "商业"),

    ("摄影风格与技法", "题材风格"): ("风格", "题材"),
    ("摄影风格与技法", "技术手法"): ("风格", "技法"),
    ("摄影风格与技法", "构图"): ("风格", "构图"),
    ("摄影风格与技法", "光线"): ("风格", "光线"),

    ("材质与纹理", "材质"): ("材质", "质感"),
    ("材质与纹理", "背景"): ("材质", "背景"),
}

# 「建筑类型」按词拆成四个二级（原表 15 个词一锅炖，当筛选器太粗）
BUILDING_SPLIT = {
    "房屋": {"住宅", "别墅", "四合院"},
    "教堂寺庙": {"寺庙", "教堂", "清真寺"},
    "古迹": {"宫殿", "城墙", "牌坊", "亭子", "园林", "城堡"},
    "塔桥": {"塔", "桥梁", "灯塔"},
}

# ── 删除（共 50 条）────────────────────────────────────────────
DROP = set()
# ① 抽象情感：CLIP 会给出「看起来合理」的分数，但基本是伪相关（柔和光≈宁静）
DROP |= {"爱", "幸福", "欢乐", "宁静", "浪漫", "希望", "自由",
         "悲伤", "孤独", "怀旧", "力量", "勇气", "友谊", "亲情", "梦想"}
# ② 时间与季节：EXIF 拍摄时间 + GPS 就能算，别让模型猜（夜晚≈夜景，重复）
DROP |= {"春天", "夏天", "秋天", "冬天", "清晨", "正午", "午后", "傍晚", "夜晚"}
# ③ 题材摄影：与内容标签重复，且「是不是摄影题材」CLIP 分辨不了
DROP |= {"生态摄影", "静物摄影", "美食摄影", "建筑摄影"}
# ④ 无视觉定义的行业词
DROP |= {"金融", "物流", "办公", "购物"}
# ⑤ 与已有标签重复 / 无信息量
DROP |= {"夜景摄影", "摄影", "电竞", "农业", "表情"}
# ⑥ 无视觉符号的日期类节日（CLIP 打不出来，靠 EXIF 日期 + 日历更准）
DROP |= {"重阳节", "妇女节", "劳动节", "母亲节", "父亲节", "教师节"}
# ⑨ 实测误判高（「元旦」25 张全是烟花/夜景的误命中），且日期可由 EXIF 推算
DROP |= {"元旦", "情人节", "儿童节", "七夕", "元宵节"}
# ⑩ 构图类：实测「三分法/对称构图」各命中 20/17 张，是高分**吸引子**，且没人按构图搜图
DROP |= {"三分法", "对称构图", "引导线", "留白", "俯拍"}
# ⑦ 低频细类：相册出现率低，白占词表名额
DROP |= {"猪", "骆驼", "瓢虫", "蜗牛", "蜻蜓", "大理石", "铁锈"}
# ⑧ 与一级类目重复（人物/风景已经能表达，不必再占一个风格位）
DROP |= {"人像摄影", "风光摄影"}

# ── 新增（共 32 条）──────────────────────────────────────────────
ADD = [
    # 截图与文档（原表完全没有，但电脑相册里占比很高）
    ("文档", "截图", "截图", "截屏;屏幕截图", "screenshot"),
    ("文档", "截图", "网页截图", "网站截图", "webpage screenshot"),
    ("文档", "截图", "聊天截图", "对话截图", "chat screenshot"),
    ("文档", "截图", "软件界面", "应用界面", "software interface"),
    ("文档", "截图", "游戏画面", "游戏截图", "game screen"),
    ("文档", "办公文档", "文档", "文件", "document"),
    ("文档", "办公文档", "表格", "电子表格", "spreadsheet"),
    ("文档", "办公文档", "白板", "白板板书", "whiteboard"),
    ("文档", "单据", "票据", "收据", "receipt"),
    # 证件类（用户点名要）
    ("文档", "证件", "证件", "证明文件", "document card"),
    ("文档", "证件", "身份证", "证件原件", "ID card"),
    ("文档", "证件", "护照", "签证", "passport"),
    ("文档", "证件", "驾驶证", "驾照", "driver license"),
    ("文档", "证件", "毕业证", "学位证", "diploma"),
    ("文档", "证件", "证书", "奖状", "certificate"),
    ("文档", "证件", "名片", "名片夹", "business card"),
    ("文档", "证件", "银行卡", "信用卡", "bank card"),
    # 单据
    ("文档", "单据", "发票", "报销单", "invoice"),
    ("文档", "单据", "合同", "协议", "contract"),
    ("文档", "单据", "账单", "对账单", "bill"),
    ("文档", "单据", "机票", "登机牌", "boarding pass"),
    # 风格补齐：插画向（原表 28 个风格全是传统摄影风格）
    ("风格", "题材", "二次元", "动画风;动漫风", "anime"),
    ("风格", "题材", "日系", "日式;和风", "japanese style"),
    ("风格", "题材", "赛博朋克", "未来都市", "cyberpunk"),
    ("风格", "题材", "像素风", "像素画", "pixel art"),
    ("风格", "题材", "小清新", "清新;清淡", "fresh style"),
    # 数码与日常补缺
    ("物品", "数码", "键盘", "机械键盘", "keyboard"),
    ("物品", "数码", "显示器", "屏幕;电脑屏幕", "monitor"),
    ("物品", "数码", "无人机", "航拍机", "drone"),
    ("美食", "饮品", "奶茶", "珍珠奶茶", "milk tea"),
    ("美食", "中餐", "便当", "盒饭", "bento"),
    ("交通", "枢纽", "机场", "航站楼", "airport"),
]


def load_src():
    with SRC.open(encoding="utf-8-sig", newline="") as f:
        return [dict(r) for r in csv.DictReader(f)]


# ── 合并（别名 → 主词）：CLIP 分不清的上下位/近义词，只留一个 ─────────
# ⭐ 合并**不是删**：别名会追加进主词的「同义词」，prompt 集成时一起跑，
#    所以「京剧」的图仍然算「戏曲」，召回不受影响 —— 只是不再单独占一个标签位。
MERGE = {
    # 活动：项目名 vs 运动种类，图上看不出差别
    "马拉松": "跑步", "京剧": "戏曲", "街舞": "舞蹈",
    "围棋": "棋牌", "象棋": "棋牌",
    "编织": "手工", "刺绣": "手工", "剪纸": "手工",
    "卡拉OK": "表演",
    # 运动：冷门技击归一
    "柔道": "武术", "击剑": "武术", "跆拳道": "武术",
    # 交通：车型细分 CLIP 靠不住，收到「汽车/火车」这层
    "高铁": "火车", "有轨电车": "地铁",
    "跑车": "汽车", "越野车": "汽车", "救护车": "汽车", "消防车": "汽车", "警车": "汽车",
    "直升机": "飞机", "游艇": "帆船",
    # 城市：同一条老街/夜景拍法
    "天际线": "城市风光", "胡同": "老街", "四合院": "老街", "亭子": "园林",
    # 植物：树种细分（相机看不出来，只有植物学家分得清）
    "松树": "树", "柳树": "树", "枫树": "树", "银杏": "树", "椰树": "树",
    "树林": "森林", "竹林": "森林", "热带雨林": "森林",
    # 花卉：除强视觉的几种，其余收到「花」
    "牡丹": "花", "梅花": "花", "兰花": "花", "桃花": "花",
    "蒲公英": "花", "百合": "花", "杜鹃": "花", "桂花": "花",
    # 美食
    "面食": "面条", "包子": "饺子",
    # 物品
    "笔记本": "电脑", "高跟鞋": "鞋", "香水": "化妆品",
    # 文档
    "软件界面": "截图", "名片": "证件", "驾驶证": "证件", "毕业证": "证书", "账单": "发票",
    # 人物
    "夫妻": "情侣", "行人": "人群", "半身照": "特写", "消防员": "警察", "艺人": "舞者",
    # 风景
    "高原": "山地", "丘陵": "山地", "平原": "田野", "戈壁": "沙漠",
    "海岸": "海洋", "溪流": "河流",
    # 天气：都看云量/降水，分不到这么细
    "多云": "阴天", "暴雨": "雨天", "冰雹": "雨天", "雾霾": "雾",
    "黄昏": "日落", "黎明": "日出",
    # 工业/材质：细分类别视觉证据不足
    "车间": "工厂", "生产线": "工厂", "石纹": "纹理", "布料纹理": "纹理",
    "写作": "阅读",
    # 第二轮：演唱会/音乐会/卡拉OK 在图上都是「舞台+人+灯」
    "卡拉OK": "演出", "演唱会": "演出", "音乐会": "演出",
    # 动物：亚种归一
    "豹": "老虎", "北极熊": "熊",
    # 物品 / 文化 / 美食
    "戒指": "珠宝", "戏剧": "戏曲", "杂技": "舞蹈",
    "苹果": "水果", "香蕉": "水果", "葡萄": "水果", "下午茶": "甜品", "动漫": "漫画",
    # 风格：长曝光 vs 延时、黄金时刻 vs 蓝调时刻，图上分不出
    "延时摄影": "长曝光",
    # ── 用户指定（2026-10-20 三组）──
    "订婚": "婚礼", "求婚": "婚礼", "婚纱照": "婚礼",
    "派对": "聚会",
    "朝霞": "日出", "晚霞": "日落",
    # ── 同类近义：图上分不出，留常用那个 ──
    "山峰": "山地",
    "银河": "星空",
    "草坪": "草地", "盆栽": "绿植", "仙人掌": "多肉植物",
    "店铺": "商场", "会议室": "办公室",
    "票据": "发票", "网页截图": "截图",
    "大笑": "微笑", "全家福": "合影",
    "庙会": "集市", "灯会": "集市",
    # ── 第二轮扫描（同组内语义重复，模型分不出）──
    "亲子": "家庭", "纪念日": "生日",
    "开幕式": "演出", "颁奖典礼": "演出",
    "遗址": "古迹", "美术馆": "博物馆", "古镇": "老街",
    "项链": "珠宝", "田径": "跑步", "收割": "耕作",
    "背包旅行": "旅行",
}

# ── 挪类（映射后个别词归错桶，做最后校正）──────────────────────
MOVE = {
    # "商业与服务" 被删到只剩「实验室」，而实验室是工业场景
    "实验室": ("行业", "工业"),
    # 电影是内容不是表演形式
    "电影": ("文化", "爱好"),
}

# ── 改名（同一条目换个更简练的说法）────────────────────────────
RENAME = {
    "纪实摄影": "纪实",
    "街头摄影": "街拍",
    "黑白摄影": "黑白",
    "极简主义": "极简",
}

# ── 新增（二次元相关，用户点名）──────────────────────────────────
ADD_ANIME = [
    ("风格", "题材", "卡通", "动画片;卡通画", "cartoon"),
    ("风格", "题材", "漫画", "连环画;漫改", "comic"),
    ("风格", "题材", "cosplay", "角色扮演;cos", "cosplay"),
    ("物品", "杂项", "手办", "模型摆件;figure", "figure"),
    ("物品", "杂项", "高达", "机甲模型;敢达", "gundam"),
    ("物品", "杂项", "模型", "拼装模型;模型玩具", "model kit"),
    # 机构场所：原表只有「教室」（室内），没有学校本身
    ("城市", "公共设施", "学校", "校园;大学;中学;小学", "school"),
    # 军事：枪械、冷兵器、装甲车、军舰在图上辨识度高，值得单独一类
    ("军事", "兵器", "武器", "枪械;兵器", "weapon"),
    ("军事", "兵器", "刀剑", "刀具;长剑;武士刀", "sword"),
    ("军事", "军备", "坦克", "装甲车;战车", "tank"),
    ("军事", "军备", "军舰", "战舰;航空母舰", "warship"),
]


def map_row(r):
    """原表一行 → 新类目。建筑类型特殊处理（按词拆分）。"""
    l1, l2 = r["一级类目"], r["二级类目"]
    word = r["主题词(首选词)"]
    if l1 == "城市与建筑" and l2 == "建筑类型":
        for new_l2, words in BUILDING_SPLIT.items():
            if word in words:
                return "建筑", new_l2
        return "建筑", "古迹"
    if (l1, l2) not in SUB_MAP:
        raise KeyError(f"未映射的类目：{l1} / {l2}")
    return SUB_MAP[(l1, l2)]


def resolve(word, known):
    """沿 MERGE 链走到主词；主词必须存在。"""
    hops = 0
    while word in MERGE and hops < 8:
        word = MERGE[word]
        hops += 1
    return word if word in known else None


def build():
    src = load_src()
    src_words = {r["主题词(首选词)"] for r in src}
    missing = DROP - src_words
    if missing:
        print("⚠️ 删除名单里有表中不存在的词：", "、".join(sorted(missing)))

    add_rows = ADD + ADD_ANIME
    add_words = {a[2] for a in add_rows}
    known = (src_words - DROP) | add_words

    # ① 解析合并链（别名 → 主词）。别名可以是原表的词，也可以是新增词
    alias_to = {}
    for word, target in MERGE.items():
        if word in DROP or word not in known:
            continue
        canon = resolve(word, known)
        if canon is None or canon == word:
            print("⚠️ 合并目标不存在，跳过：", word, "→", target)
            continue
        alias_to[word] = canon

    # ② 原表：删 DROP、跳别名
    out = []
    for r in src:
        word = r["主题词(首选词)"]
        if word in DROP or word in alias_to:
            continue
        l1, l2 = map_row(r)
        if word in MOVE:
            l1, l2 = MOVE[word]
        word = RENAME.get(word, word)
        out.append({"编码": r["编码"], "一级类目": l1, "二级类目": l2, "主题词": word,
                    "同义词": r["同义词(非首选词)"], "英文": r["英文对照"]})

    # ③ 新增（同样跳别名）
    have = {o["主题词"] for o in out}
    idx = 0
    for l1, l2, word, syn, en in add_rows:
        if word in have:
            print("⚠️ 新增词已存在，跳过：", word)
            continue
        if word in alias_to:
            continue
        idx += 1
        out.append({"编码": f"X{idx:03d}", "一级类目": l1, "二级类目": l2, "主题词": word,
                    "同义词": syn, "英文": en})
        have.add(word)

    # ④ 统一把别名并进主词的同义词（source / 新增都覆盖）
    by_word = {o["主题词"]: o for o in out}
    merged = 0
    for alias, canon in alias_to.items():
        row = by_word.get(canon)
        if row is None:
            continue
        parts = [x for x in row["同义词"].split(";") if x]
        if alias not in parts:
            parts.append(alias)
            row["同义词"] = ";".join(parts)
            merged += 1

    # ⑤ 按 L1_ORDER 排序
    rank = {k: i for i, k in enumerate(L1_ORDER)}
    out.sort(key=lambda o: rank.get(o["一级类目"], 999))
    print(f"合并 {merged} 条别名进同义词")
    return out


def write_csv(rows):
    path = HERE / "zh_tags.csv"
    with path.open("w", encoding="utf-8-sig", newline="") as f:
        w = csv.writer(f)
        w.writerow(["编码", "一级类目", "二级类目", "主题词", "同义词", "英文对照"])
        for r in rows:
            w.writerow([r["编码"], r["一级类目"], r["二级类目"], r["主题词"], r["同义词"], r["英文"]])
    return path


CSS = """
:root{--bg:#16161a;--panel:#1d1d22;--line:#2c2c33;--txt:#e7e7ec;--dim:#8b8b96;--accent:#5b9cf8}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--txt);font:14px/1.55 "Segoe UI","Microsoft YaHei",system-ui,sans-serif}
header{position:sticky;top:0;z-index:5;background:linear-gradient(#16161a,#16161af5);
  border-bottom:1px solid var(--line);padding:16px 26px 12px}
h1{margin:0;font-size:19px;font-weight:600}
.sub{color:var(--dim);font-size:12.5px;margin-top:3px}
.stats{display:flex;gap:16px;margin-top:9px;font-size:12.5px;color:var(--dim)}
.stats b{color:var(--txt);font-size:15px;margin-right:3px}
.tools{display:flex;gap:8px;margin-top:10px;flex-wrap:wrap;align-items:center}
input[type=search]{flex:1;min-width:240px;padding:7px 11px;border-radius:8px;border:1px solid var(--line);
  background:var(--panel);color:var(--txt);outline:none}
input[type=search]:focus{border-color:var(--accent)}
.chip{padding:3px 10px;border-radius:999px;border:1px solid var(--line);background:var(--panel);
  color:var(--dim);font-size:12px;cursor:pointer}
.chip.on{border-color:var(--accent);color:var(--accent);background:#1b2433}
main{padding:16px 26px 60px;display:flex;flex-direction:column;gap:12px}
.cat{background:var(--panel);border:1px solid var(--line);border-radius:10px;padding:11px 13px}
.cat h2{margin:0 0 8px;font-size:13.5px;font-weight:600}
.cat h2 .n{color:var(--dim);font-size:11.5px;font-weight:400;margin-left:7px}
.sub2{color:var(--dim);font-size:11px;margin:6px 0 4px}
.tags{display:flex;flex-wrap:wrap;gap:5px}
.tag{display:inline-flex;align-items:baseline;gap:5px;padding:3px 9px;border-radius:999px;
  background:#23232a;border:1px solid #2f2f37;font-size:12.5px;color:#d7d7de;cursor:default}
.tag .en{font-size:10px;color:#6f6f7a}
footer{padding:0 26px 40px;color:var(--dim);font-size:12px;line-height:1.7}
code{background:#23232a;padding:1px 5px;border-radius:4px}
"""

JS = """
const q=document.getElementById('q');
const chips=[...document.querySelectorAll('.chip[data-cat]')];
let cat='';
function apply(){
  const kw=q.value.trim().toLowerCase();
  let shown=0;
  document.querySelectorAll('.cat').forEach(sec=>{
    const okCat=!cat||sec.dataset.cat===cat;
    let local=0;
    sec.querySelectorAll('.tag').forEach(t=>{
      const hit=okCat&&(!kw||t.dataset.s.includes(kw));
      t.style.display=hit?'':'none';
      if(hit)local++;
    });
    sec.querySelectorAll('.sub2').forEach(h=>{
      let any=false, n=h.nextElementSibling;
      while(n&&!n.classList.contains('sub2')){
        if(n.classList.contains('tags')&&[...n.children].some(c=>c.style.display!=='none'))any=true;
        n=n.nextElementSibling;
      }
      h.style.display=any?'':'none';
    });
    sec.style.display=local?'':'none';
    sec.querySelector('.n').textContent=local+' 条';
    shown+=local;
  });
  document.getElementById('hit').textContent=shown;
}
q.addEventListener('input',apply);
chips.forEach(c=>c.addEventListener('click',()=>{
  chips.forEach(x=>x.classList.remove('on')); c.classList.add('on');
  cat=c.dataset.cat; q.value=''; apply(); window.scrollTo({top:0,behavior:'smooth'});
}));
apply();
"""


def write_html(rows):
    l1s = [k for k in L1_ORDER if any(r["一级类目"] == k for r in rows)]
    parts = []
    for l1 in l1s:
        sub = collections.OrderedDict()
        for r in rows:
            if r["一级类目"] == l1:
                sub.setdefault(r["二级类目"], []).append(r)
        total = sum(len(v) for v in sub.values())
        inner = [f'<h2>{html.escape(l1)}<span class="n">{total} 条</span></h2>']
        for l2, items in sub.items():
            chips = "".join(
                f'<span class="tag" data-s="{html.escape((r["主题词"] + " " + r["同义词"] + " " + r["英文"]).lower())}"'
                f' title="{html.escape(r["同义词"] or "—")} · {html.escape(r["英文"])}">'
                f'{html.escape(r["主题词"])}<span class="en">{html.escape(r["英文"])}</span></span>'
                for r in items
            )
            inner.append(f'<div class="sub2">{html.escape(l2)}</div><div class="tags">{chips}</div>')
        parts.append(f'<section class="cat" data-cat="{html.escape(l1)}">{"".join(inner)}</section>')

    chips = '<button class="chip on" data-cat="">全部</button>' + "".join(
        f'<button class="chip" data-cat="{html.escape(l1)}">{html.escape(l1)}</button>' for l1 in l1s)
    n_l2 = len({r["二级类目"] for r in rows})
    n_syn = sum(1 for r in rows if r["同义词"])

    return f"""<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>照片库中文主题词表 · {len(rows)} 条</title><style>{CSS}</style></head>
<body>
<header>
  <h1>照片库中文主题词表 · <span id="hit">{len(rows)}</span> 条</h1>
  <div class="sub">一级/二级用日常词，直接当筛选器；按 CN-CLIP 判别力取舍</div>
  <div class="stats"><span><b>{len(l1s)}</b>一级</span><span><b>{n_l2}</b>二级</span>
    <span><b>{len(rows)}</b>主题词</span><span><b>{n_syn}</b>带同义词</span></div>
  <div class="tools"><input id="q" type="search" placeholder="搜中文 / 同义词 / 英文（如「护照 passport」「夜」）…"></div>
  <div class="tools">{chips}</div>
</header>
<main>{''.join(parts)}</main>
<footer>
  打标：每张取 Top-5 且相似度 ≥ 0.55；同义词用于 prompt 集成（同一标签 2–3 条模板取平均）<br>
  时间/季节/地点/机型来自 EXIF，不进 AI 词表 · 数据 <code>crates/pv-ai/vocab/zh_tags.csv</code>
</footer>
<script>{JS}</script>
</body></html>
"""


def main():
    rows = build()
    words = [r["主题词"] for r in rows]
    dup = [w for w, c in collections.Counter(words).items() if c > 1]
    print(f"总量 {len(rows)}｜一级 {len({r['一级类目'] for r in rows})}｜"
          f"二级 {len({r['二级类目'] for r in rows})}")
    if dup:
        print("⚠️ 重复：", "、".join(dup))

    p = write_csv(rows)
    h = HERE / "词表预览.html"
    h.write_text(write_html(rows), encoding="utf-8")
    print("写出：", p.name, h.name)

    by_l1 = collections.Counter(r["一级类目"] for r in rows)
    print("一级分布：", " / ".join(f"{k}{v}" for k, v in by_l1.items()))
    print("文档类：", "、".join(r["主题词"] for r in rows if r["一级类目"] == "文档"))


if __name__ == "__main__":
    main()

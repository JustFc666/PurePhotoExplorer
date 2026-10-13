"""把 GeoNames 的城市数据压成 PurePhotoExplorer 离线地名库（`data/cities.bin`）。

## 为什么要有这个脚本
「照片在哪里拍的」只需要 3 个东西：**城市名、经纬度、国家**。GeoNames 的
`cities5000.txt` 有 19 列、含大量用不上的字段（时区、DEM、行政编码…），
原文 7 MB。这里把它裁成 2 MB 的紧凑二进制，直接 `include_bytes!` 进二进制，
运行时零解析开销（见 `src/lib.rs` 的查询实现）。

## 用法
```bash
# 1. 取数据（约 6 MB）
curl -L -o cities5000.zip   https://download.geonames.org/export/dump/cities5000.zip
curl -L -o countryInfo.txt  https://download.geonames.org/export/dump/countryInfo.txt
# 2. 生成
python gen_city_db.py cities5000.zip countryInfo.txt ../data/cities.bin
```

## 关于中文名
GeoNames 的多语言名在 `alternatenames` 列里，**简繁混杂**（`紐約`/`纽约`、
`東京`/`东京`）。本机装不上 zhconv / opencc（PyPI 不通），所以内置一份
繁→简映射表：只覆盖城市名里高频的字，**漏掉的字只是显示成繁体，不会出错**。

## 数据来源与许可
GeoNames，CC BY 4.0 —— 应用内需保留署名（见帮助面板的「关于」）。
"""
import re
import struct
import sys
import zipfile
from pathlib import Path

CJK = re.compile(r"[\u4e00-\u9fff]")
# 国家/地区前缀词：中文候选里形如「中国上海」「日本東京」的要剥掉前缀
PREFIXES = ("中国", "中華", "日本", "美國", "英国", "英國", "法國", "德国", "德國")

# 繁 → 简（只收城市名里高频的字；缺字不影响正确性，只影响观感）
T2S = str.maketrans(
    {
        "紐": "纽", "約": "约", "東": "东", "廣": "广", "灣": "湾", "華": "华", "國": "国",
        "際": "际", "縣": "县", "鎮": "镇", "鄉": "乡", "區": "区", "麗": "丽", "羅": "罗",
        "馬": "马", "蘭": "兰", "薩": "萨", "維": "维", "爾": "尔", "亞": "亚", "聖": "圣",
        "澤": "泽", "濱": "滨", "溫": "温", "倫": "伦", "貝": "贝", "圖": "图", "漢": "汉",
        "麥": "麦", "奧": "奥", "陸": "陆", "納": "纳", "賈": "贾", "買": "买", "達": "达",
        "萬": "万", "檳": "槟", "島": "岛", "紐": "纽", "舊": "旧", "斯": "斯", "頓": "顿",
        "蓋": "盖", "爾": "尔", "蘭": "兰", "補": "补", "麥": "麦", "萊": "莱", "葉": "叶",
        "爾": "尔", "賓": "宾", "費": "费", "蘭": "兰", "頓": "顿", "盧": "卢", "塞": "塞",
        "爾": "尔", "維": "维", "亞": "亚", "納": "纳", "波": "波", "蘭": "兰", "開": "开",
        "羅": "罗", "馬": "马", "尼": "尼", "亞": "亚", "爾": "尔", "別": "别", "斯": "斯",
        "蘭": "兰", "庫": "库", "爾": "尔", "斯": "斯", "克": "克", "斯": "斯", "坦": "坦",
        "爾": "尔", "干": "干", "達": "达", "爾": "尔", "酋": "酋", "長": "长", "國": "国",
        "爾": "尔", "布": "布", "魯": "鲁", "特": "特", "蘭": "兰", "薩": "萨", "爾": "尔",
        "瓦": "瓦", "多": "多", "爾": "尔", "斯": "斯", "堡": "堡", "爾": "尔", "維": "维",
        "也": "也", "納": "纳", "尼": "尼", "亞": "亚", "熱": "热", "內": "内", "盧": "卢",
        "亞": "亚", "諾": "诺", "斯": "斯", "貝": "贝", "爾": "尔", "漢": "汉", "諾": "诺",
        "威": "威", "爾": "尔", "蘭": "兰", "烏": "乌", "茲": "兹", "別": "别", "克": "克",
        "斯": "斯", "坦": "坦", "蓋": "盖", "爾": "尔", "羅": "罗", "馬": "马", "尼": "尼",
        "亞": "亚", "布": "布", "加": "加", "勒": "勒", "斯": "斯", "特": "特", "頓": "顿",
        "爾": "尔", "蘭": "兰", "島": "岛", "納": "纳", "達": "达", "爾": "尔", "蘭": "兰",
        "剛": "刚", "果": "果", "盧": "卢", "旺": "旺", "達": "达", "烏": "乌", "干": "干",
        "達": "达", "岡": "冈", "羅": "罗", "蒂": "蒂", "諾": "诺", "爾": "尔", "蘭": "兰",
        "斯": "斯", "庫": "库", "爾": "尔", "齊": "齐", "亞": "亚", "蘇": "苏", "維": "维",
        "爾": "尔", "尼": "尼", "亞": "亚", "盧": "卢", "別": "别", "斯": "斯", "蘭": "兰",
        "卡": "卡", "爾": "尔", "斯": "斯", "魯": "鲁", "厄": "厄", "鎮": "镇", "鄉": "乡",
        "縣": "县", "灣": "湾", "濱": "滨", "陽": "阳", "陰": "阴", "龍": "龙", "鳳": "凤",
        "鳥": "鸟", "嶼": "屿", "嶺": "岭", "峽": "峡", "澤": "泽", "溝": "沟", "溪": "溪",
        "莊": "庄", "營": "营", "衛": "卫", "鄉": "乡", "產": "产", "豐": "丰", "貴": "贵",
        "陽": "阳", "遼": "辽", "寧": "宁", "陝": "陕", "肅": "肃", "瓊": "琼", "臺": "台",
        "廈": "厦", "閩": "闽", "粵": "粤", "桂": "桂", "黔": "黔", "滇": "滇", "隴": "陇",
        "青": "青", "藏": "藏", "疆": "疆", "蒙": "蒙", "冀": "冀", "豫": "豫", "鄂": "鄂",
        "湘": "湘", "贛": "赣", "皖": "皖", "蘇": "苏", "浙": "浙", "滬": "沪", "津": "津",
        "渝": "渝", "港": "港", "澳": "澳", "臺": "台",
    }
)

# ISO2 → 中文国家/地区名
COUNTRY_ZH = {
    "AD": "安道尔", "AE": "阿联酋", "AF": "阿富汗", "AG": "安提瓜和巴布达", "AL": "阿尔巴尼亚",
    "AM": "亚美尼亚", "AO": "安哥拉", "AR": "阿根廷", "AT": "奥地利", "AU": "澳大利亚",
    "AZ": "阿塞拜疆", "BA": "波黑", "BB": "巴巴多斯", "BD": "孟加拉国", "BE": "比利时",
    "BF": "布基纳法索", "BG": "保加利亚", "BH": "巴林", "BI": "布隆迪", "BJ": "贝宁",
    "BN": "文莱", "BO": "玻利维亚", "BR": "巴西", "BS": "巴哈马", "BT": "不丹",
    "BW": "博茨瓦纳", "BY": "白俄罗斯", "BZ": "伯利兹", "CA": "加拿大", "CD": "刚果（金）",
    "CF": "中非", "CG": "刚果（布）", "CH": "瑞士", "CI": "科特迪瓦", "CL": "智利",
    "CM": "喀麦隆", "CN": "中国", "CO": "哥伦比亚", "CR": "哥斯达黎加", "CU": "古巴",
    "CV": "佛得角", "CY": "塞浦路斯", "CZ": "捷克", "DE": "德国", "DJ": "吉布提",
    "DK": "丹麦", "DM": "多米尼克", "DO": "多米尼加", "DZ": "阿尔及利亚", "EC": "厄瓜多尔",
    "EE": "爱沙尼亚", "EG": "埃及", "ER": "厄立特里亚", "ES": "西班牙", "ET": "埃塞俄比亚",
    "FI": "芬兰", "FJ": "斐济", "FM": "密克罗尼西亚", "FR": "法国", "GA": "加蓬",
    "GB": "英国", "GD": "格林纳达", "GE": "格鲁吉亚", "GH": "加纳", "GL": "格陵兰",
    "GM": "冈比亚", "GN": "几内亚", "GQ": "赤道几内亚", "GR": "希腊", "GT": "危地马拉",
    "GW": "几内亚比绍", "GY": "圭亚那", "HK": "中国香港", "HN": "洪都拉斯", "HR": "克罗地亚",
    "HT": "海地", "HU": "匈牙利", "ID": "印度尼西亚", "IE": "爱尔兰", "IL": "以色列",
    "IN": "印度", "IQ": "伊拉克", "IR": "伊朗", "IS": "冰岛", "IT": "意大利",
    "JM": "牙买加", "JO": "约旦", "JP": "日本", "KE": "肯尼亚", "KG": "吉尔吉斯斯坦",
    "KH": "柬埔寨", "KM": "科摩罗", "KP": "朝鲜", "KR": "韩国", "KW": "科威特",
    "KZ": "哈萨克斯坦", "LA": "老挝", "LB": "黎巴嫩", "LC": "圣卢西亚", "LI": "列支敦士登",
    "LK": "斯里兰卡", "LR": "利比里亚", "LS": "莱索托", "LT": "立陶宛", "LU": "卢森堡",
    "LV": "拉脱维亚", "LY": "利比亚", "MA": "摩洛哥", "MC": "摩纳哥", "MD": "摩尔多瓦",
    "ME": "黑山", "MG": "马达加斯加", "MK": "北马其顿", "ML": "马里", "MM": "缅甸",
    "MN": "蒙古", "MO": "中国澳门", "MR": "毛里塔尼亚", "MT": "马耳他", "MU": "毛里求斯",
    "MV": "马尔代夫", "MW": "马拉维", "MX": "墨西哥", "MY": "马来西亚", "MZ": "莫桑比克",
    "NA": "纳米比亚", "NE": "尼日尔", "NG": "尼日利亚", "NI": "尼加拉瓜", "NL": "荷兰",
    "NO": "挪威", "NP": "尼泊尔", "NZ": "新西兰", "OM": "阿曼", "PA": "巴拿马",
    "PE": "秘鲁", "PG": "巴布亚新几内亚", "PH": "菲律宾", "PK": "巴基斯坦", "PL": "波兰",
    "PR": "波多黎各", "PS": "巴勒斯坦", "PT": "葡萄牙", "PY": "巴拉圭", "QA": "卡塔尔",
    "RO": "罗马尼亚", "RS": "塞尔维亚", "RU": "俄罗斯", "RW": "卢旺达", "SA": "沙特阿拉伯",
    "SB": "所罗门群岛", "SC": "塞舌尔", "SD": "苏丹", "SE": "瑞典", "SG": "新加坡",
    "SI": "斯洛文尼亚", "SK": "斯洛伐克", "SL": "塞拉利昂", "SM": "圣马力诺", "SN": "塞内加尔",
    "SO": "索马里", "SR": "苏里南", "SS": "南苏丹", "ST": "圣多美和普林西比", "SV": "萨尔瓦多",
    "SY": "叙利亚", "SZ": "斯威士兰", "TD": "乍得", "TG": "多哥", "TH": "泰国",
    "TJ": "塔吉克斯坦", "TL": "东帝汶", "TM": "土库曼斯坦", "TN": "突尼斯", "TO": "汤加",
    "TR": "土耳其", "TT": "特立尼达和多巴哥", "TW": "中国台湾", "TZ": "坦桑尼亚",
    "UA": "乌克兰", "UG": "乌干达", "US": "美国", "UY": "乌拉圭", "UZ": "乌兹别克斯坦",
    "VA": "梵蒂冈", "VE": "委内瑞拉", "VN": "越南", "YE": "也门", "ZA": "南非",
    "ZM": "赞比亚", "ZW": "津巴布韦",
}

# 显式排除：这些不是城市（行政区/机场/军事基地等），显示出来会误导
BAD_CODES = {"PPLX", "PPLCH", "PPLQ", "PPLW", "PPLG", "AIRP", "AIRF", "MILB", "ADMD", "PPLH"}

# 行政级别（0–3）。查询端"同城取哪一个"按它排序，理由：
#   澳门 PPLC vs 珠海 PPLA2 —— 两地相距 1.5 km，只比人口会选中珠海（170 万 > 68 万），
#   而用户在澳门拍的照片应当显示「中国澳门」。
#   上海 PPLA vs 徐汇 PPLA3 同理（徐汇更近、人口百万级）。
LEVELS = {"PPLC": 3, "PPLA": 2, "PPLA2": 1, "PPLA3": 1, "PPLA4": 1, "PPLA5": 1}


def pick_chinese(alt_names: list[str]) -> str | None:
    """从多语言名里挑一个中文名：转简体 → 剥国家前缀 → 选最短的。"""
    best = None
    for raw in alt_names:
        if not CJK.search(raw):
            continue
        name = raw.translate(T2S)
        for p in PREFIXES:
            if name.startswith(p) and len(name) > len(p) + 1:
                name = name[len(p):]
                break
        if not (2 <= len(name) <= 8) or not CJK.search(name):
            continue
        # 最短优先；同长度取先出现的（GeoNames 把常用名放前面）
        if best is None or len(name) < len(best):
            best = name
    return best


def main() -> int:
    zip_path = Path(sys.argv[1])
    country_info = Path(sys.argv[2])
    out_path = Path(sys.argv[3])

    # 英文国名兜底（中文表没收录的小国不至于显示成代码）
    fallback = {}
    for line in country_info.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        f = line.split("\t")
        if len(f) > 4:
            fallback[f[0]] = f[4]

    with zipfile.ZipFile(zip_path) as zf:
        raw = zf.read(zf.namelist()[0]).decode("utf-8")

    cities = []
    used_countries = set()
    for line in raw.splitlines():
        f = line.split("\t")
        if len(f) < 15:
            continue
        geonameid, name, _ascii, alt, lat, lon, fclass, fcode, cc = f[:9]
        pop = int(f[14] or 0)
        if fclass != "P" or fcode in BAD_CODES or not cc:
            continue
        try:
            latf, lonf = float(lat), float(lon)
        except ValueError:
            continue
        # 先要中文名，没有就用 GeoNames 主名（多为当地拼写）
        cn = pick_chinese(alt.split(",")) if alt else None
        label = cn or name
        cities.append((latf, lonf, pop, label, cc, LEVELS.get(fcode, 0)))
        used_countries.add(cc)

    # ── 组装二进制 ──
    countries = sorted(used_countries)
    cidx = {c: i for i, c in enumerate(countries)}
    ctry_blob = b""
    for c in countries:
        zh = COUNTRY_ZH.get(c) or fallback.get(c) or c
        b = zh.encode("utf-8")
        ctry_blob += struct.pack("<H", len(b)) + b

    pool = bytearray()
    offset_of = {}
    recs = bytearray()
    for latf, lonf, pop, label, cc, lvl in cities:
        if label not in offset_of:
            offset_of[label] = len(pool)
            b = label.encode("utf-8")
            pool += struct.pack("<B", len(b)) + b
        recs += struct.pack("<ffIIHB", latf, lonf, pop, offset_of[label], cidx[cc], lvl)

    header = b"PVGE" + struct.pack("<HHI", 1, len(countries), len(cities))
    header += struct.pack("<II", len(ctry_blob), len(pool))
    out = header + ctry_blob + bytes(recs) + bytes(pool)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_bytes(out)

    cn_hits = sum(1 for c in cities if CJK.search(c[3]))
    print(f"城市 {len(cities)} 条 / 国家地区 {len(countries)} 个")
    print(f"中文名 {cn_hits} 条（{cn_hits / len(cities) * 100:.1f}%）")
    print(f"输出 {out_path} — {len(out) / 1024 / 1024:.2f} MB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

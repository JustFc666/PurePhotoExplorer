#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""重建 `image_tags` 里的 EXIF 标签：只留 **相机机型 + 地点**。

背景：早期版本还写了品牌（make）和镜头（lens），菜单里出现
`Apple` / `Apple iPhone 15 Pro Max back triple camera ...` 这种又长又重复的条目。
本脚本把 `source='exif'` 的挂载全部清掉，按当前规则重建，顺带清理孤立标签。

用法：`python tools/ai/fix_exif_tags.py`（应用需关闭，避免写锁冲突）
"""

import os
import sqlite3

DB = os.path.expandvars(r"%LOCALAPPDATA%\PurePhotoExplorer\library.db")

con = sqlite3.connect(DB)
cur = con.cursor()

n_old = cur.execute("SELECT COUNT(*) FROM image_tags WHERE source='exif'").fetchone()[0]
cur.execute("DELETE FROM image_tags WHERE source='exif'")

rows = cur.execute("""
    SELECT i.id, e.model, p.place
      FROM images i
      LEFT JOIN image_exif e ON e.image_id = i.id
      LEFT JOIN image_places p ON p.image_id = i.id
""").fetchall()

added = 0
for image_id, model, place in rows:
    for name in (model, place):
        if not name:
            continue
        cur.execute("INSERT OR IGNORE INTO tags(name, kind) VALUES (?, 'exif')", (name,))
        tag_id = cur.execute("SELECT id FROM tags WHERE name = ?", (name,)).fetchone()[0]
        cur.execute(
            "INSERT OR REPLACE INTO image_tags(image_id, tag_id, source, confidence) VALUES (?,?,'exif',NULL)",
            (image_id, tag_id),
        )
        added += 1

# 清掉没有任何挂载的标签（旧版本留下的品牌/镜头）
cur.execute("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM image_tags)")
con.commit()

print(f"清掉旧 exif 挂载 {n_old} 条 → 重建 {added} 条（机型 + 地点）")
print("剩余标签总数：", cur.execute("SELECT COUNT(*) FROM tags").fetchone()[0])
con.close()

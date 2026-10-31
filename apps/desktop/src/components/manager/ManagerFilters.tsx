/**
 * 主区标题下面那一行**筛选下拉**（从 `ManagerView.tsx` 拆出，2026-11）。
 *
 * 四个下拉的顺序是用户 2026-10-24 定的：格式 → 年份 → 相机 → 地点。
 * 每行「全部」后面跟的是**该维度的总数**（不是全库张数）—— 这是刻意的：
 * 用户点进「格式」看到的是「JPEG 12,345」，切到「相机」应该看到相机的合计，
 * 一路跟着变才解释得清「为什么筛完只剩三张」。
 *
 * ⛔ 「AI 识别」按钮**不能**用全局 `.pv-btn`（高 30px）：这一行下拉都是 26px，
 * 混进来会把整行撑高 4px、下面的人像条跟着跳位。
 */

import type { ManagerCamera, ManagerYear, PlaceItem } from '../../ipc/manager'
import { FilterDropdown, FilterRow } from './FilterDropdown'
import { formatLabelOf } from './viewConfig'

export type ManagerFiltersProps = {
  /** 有可跑 AI 的素材（空图库就别放这个按钮了） */
  aiEligible: boolean
  onOpenAi: () => void

  /** 全库总张数 —— 「全部」那一行显示它 */
  statsTotal: number
  formats: Array<{ format: string; count: number }>
  activeFormats: string[]
  onToggleFormat: (format: string) => void
  onClearFormats: () => void

  years: ManagerYear[]
  activeYears: number[]
  onToggleYear: (year: number) => void
  onClearYears: () => void

  cameras: ManagerCamera[]
  activeCameras: string[]
  onToggleCamera: (model: string) => void
  onClearCameras: () => void

  places: PlaceItem[]
  activePlaces: string[]
  onTogglePlace: (name: string) => void
  onClearPlaces: () => void
}

export function ManagerFilters(props: ManagerFiltersProps) {
  const {
    aiEligible,
    onOpenAi,
    statsTotal,
    formats,
    activeFormats,
    onToggleFormat,
    onClearFormats,
    years,
    activeYears,
    onToggleYear,
    onClearYears,
    cameras,
    activeCameras,
    onToggleCamera,
    onClearCameras,
    places,
    activePlaces,
    onTogglePlace,
    onClearPlaces,
  } = props

  return (
    <div className="pv-mg-filters">
      {/*
        一键 AI 识别（2026-10-29 用户要求：放在「格式」前面）。
        空图库（没标签没人像）时用户最自然的动作是就地跑一遍，
        而不是「打开设置 → AI 分析 → 选库 → 点两个按钮」。
      */}
      {aiEligible && (
        <button
          type="button"
          // ⛔ 不用全局 `.pv-btn`：它高 30px，而筛选行的下拉是 26px，
          //    混在一起会把整行撑高 4px、人像条跟着变位。
          //    `.pv-mg-ai` 与 `.pv-mg-fbtn` 同尺寸，只换配色（主色）。
          className="pv-mg-ai"
          title="先打标，再识别人像（两步自动串行）"
          onClick={onOpenAi}
        >
          AI 识别
        </button>
      )}

      {/* 2026-10-24：标准下拉框（锚定按钮正下方、定宽、最多 13 行可滚动） */}
      <FilterDropdown
        width={130}
        active={activeFormats.length > 0}
        title="按文件格式筛选"
        label="格式"
      >
        <FilterRow
          checked={activeFormats.length === 0}
          label="全部"
          count={statsTotal}
          onSelect={onClearFormats}
        />
        {formats.map((f) => (
          <FilterRow
            key={f.format}
            checked={activeFormats.includes(f.format)}
            label={formatLabelOf(f.format)}
            count={f.count}
            onSelect={() => onToggleFormat(f.format)}
          />
        ))}
      </FilterDropdown>

      <FilterDropdown
        width={130}
        active={activeYears.length > 0}
        title="按拍摄年份筛选（EXIF 拍摄时间，没有的按入库时间）"
        label="年份"
      >
        <FilterRow
          checked={activeYears.length === 0}
          label="全部"
          count={years.reduce((a, y) => a + y.count, 0)}
          onSelect={onClearYears}
        />
        {years.map((y) => (
          <FilterRow
            key={y.year}
            checked={activeYears.includes(y.year)}
            label={String(y.year)}
            count={y.count}
            onSelect={() => onToggleYear(y.year)}
          />
        ))}
      </FilterDropdown>

      <FilterDropdown
        width={160}
        active={activeCameras.length > 0}
        title="按相机机型筛选（来自 EXIF）"
        label="相机"
      >
        <FilterRow
          checked={activeCameras.length === 0}
          label="全部"
          count={cameras.reduce((a, c) => a + c.count, 0)}
          onSelect={onClearCameras}
        />
        {cameras.map((c) => (
          <FilterRow
            key={c.model}
            checked={activeCameras.includes(c.model)}
            label={c.model}
            count={c.count}
            onSelect={() => onToggleCamera(c.model)}
          />
        ))}
      </FilterDropdown>

      <FilterDropdown
        width={160}
        active={activePlaces.length > 0}
        title="按拍摄地点筛选（离线地名库，来自 EXIF GPS）"
        label="地点"
      >
        <FilterRow
          checked={activePlaces.length === 0}
          label="全部"
          count={places.reduce((a, p) => a + p.count, 0)}
          onSelect={onClearPlaces}
        />
        {places.map((p) => (
          <FilterRow
            key={p.name}
            checked={activePlaces.includes(p.name)}
            label={p.name}
            count={p.count}
            onSelect={() => onTogglePlace(p.name)}
          />
        ))}
      </FilterDropdown>
    </div>
  )
}

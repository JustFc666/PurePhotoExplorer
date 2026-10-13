/**
 * 图片信息浮层（右下角）。
 *
 * 只显示一组平铺字段（不分「常规信息 / 拍摄参数」两段）：
 * 尺寸、修改时间、文件大小，随后是 EXIF 拍摄参数。
 *
 * 文件名与完整路径不在这里 —— 它们在底部信息栏左下角（见 `chrome/StatusBar`）。
 * 没有 EXIF 的照片（截图、导出图），拍摄参数对应的行自动消失，不留空行。
 */

import { useState, type ReactNode } from 'react'
import { formatFileSize, openExternalUrl, type ExifData, type GpsData, type ImageInfo } from '../../ipc'

/** Unix 秒 → `YYYY-MM-DD HH:mm`（本地时区，手工格式化避免受 WebView 语言环境影响） */
function formatDateTime(unixSec: number): string {
  if (!unixSec) return '未知'
  const d = new Date(unixSec * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/**
 * 相机名：厂商 + 型号。
 * 型号里常已含厂商（`Canon` + `Canon EOS R5`），此时不重复拼。
 */
function cameraName(make: string | null, model: string | null): string | null {
  const brand = (make ?? '').trim()
  const name = (model ?? '').trim()
  if (!brand) return name || null
  if (!name) return brand
  const first = brand.split(/\s+/)[0]?.toLowerCase() ?? ''
  if (first && name.toLowerCase().includes(first)) return name
  return `${brand} ${name}`
}

/**
 * 焦距：有 35mm 等效值**且与实物焦距不同**时才一并给出。
 * 全画幅机身两者必然相同，写「73 mm（等效 73 mm）」纯属噪音。
 */
function focalText(exif: ExifData): string | null {
  if (!exif.focalLength) return null
  const actual = Number.parseFloat(exif.focalLength)
  const equiv = exif.focalLength35mm
  if (!equiv || !Number.isFinite(actual) || Math.abs(actual - equiv) < 0.5) {
    return exif.focalLength
  }
  return `${exif.focalLength}（等效 ${equiv} mm）`
}

/**
 * 拍摄地点 → 地图链接。
 *
 * ⚠️ 两个易错点，改这里前务必确认：
 * 1. **顺序是 `经度,纬度`** —— 高德 URI 的 `position` 与 EXIF / 我们展示的
 *    「纬度在前」相反，写反了会指到地球另一处（且页面看起来完全正常）。
 * 2. **必须带 `coordinate=wgs84`** —— EXIF 里记的是 GPS 原始坐标（WGS-84），
 *    而国内地图底图是 GCJ-02（火星坐标），不加这个参数会有几十到几百米的偏移。
 */
function mapUrl(gps: GpsData): string {
  const name = encodeURIComponent('拍摄地点')
  return `https://uri.amap.com/marker?position=${gps.longitude},${gps.latitude}&coordinate=wgs84&name=${name}`
}

/** 展示文本：经纬度 + 海拔（海拔可选，多数设备不记录） */
function gpsText(gps: GpsData): string {
  return gps.altitude ? `${gps.display} · 海拔 ${gps.altitude}` : gps.display
}

interface InfoRow {
  label: string
  value: string | null
  /** 值后面的附加操作（目前只有「在地图中查看」） */
  action?: ReactNode
  /**
   * 值是否允许换行显示完整。
   *
   * 默认裁成一行加省略号（面板窄，镜头型号这类长文本悬停看全即可）；
   * 但经纬度不行 —— 被截掉的是**经度**，等于信息缺了一半，
   * 所以这一行放开换行（2026-10-11 实机截图发现）。
   */
  wrap?: boolean
}

interface InfoPanelProps {
  info: ImageInfo
  onClose: () => void
}

export function InfoPanel({ info, onClose }: InfoPanelProps) {
  const exif = info.exif
  const raw = info.raw
  const gps = exif?.gps ?? null
  /** 离线地名库查到的城市（无 GPS / 附近无城镇时为 null） */
  const place = info.place
  /** 打不开浏览器时把原因说给用户（多半是没设默认浏览器） */
  const [mapError, setMapError] = useState<string | null>(null)

  const openMap = () => {
    if (!gps) return
    setMapError(null)
    openExternalUrl(mapUrl(gps)).catch((err: unknown) => {
      setMapError(err instanceof Error ? err.message : String(err))
    })
  }

  // 固定顺序：尺寸 / 修改时间 / 文件大小 + RAW 信息 + EXIF 拍摄参数
  // （无值的行为空，过滤掉）
  const allRows: Array<InfoRow> = [
    // 尺寸未知（无内嵌预览的 RAW）时不显示 "0 × 0"，那是误导
    { label: '尺寸', value: info.width > 0 && info.height > 0 ? `${info.width} × ${info.height}` : null },
    { label: '修改时间', value: formatDateTime(info.modifiedAt) },
    { label: '文件大小', value: formatFileSize(info.fileSize) },
    // ── RAW 专属（仅 RAW 文件显示）──
    { label: 'RAW 格式', value: raw?.rawFormat ?? null },
    { label: '内嵌预览', value: raw ? (raw.hasEmbeddedPreview ? '可用' : '不可用') : null },
    { label: '色彩空间', value: raw?.colorSpace ?? null },
    // ── 拍摄参数 ──
    { label: '相机', value: exif ? cameraName(exif.make, exif.model) : null },
    { label: '镜头', value: exif?.lens ?? null },
    { label: '拍摄时间', value: exif?.dateTimeOriginal ?? null },
    {
      // 有离线库查到的城市就显示城市名，否则退回坐标（哪怕只有坐标也是"地点"）
      label: '拍摄地点',
      value: place ? place.label : gps ? gpsText(gps) : null,
      action: gps ? (
        <button
          className="pv-info-map-btn"
          onClick={openMap}
          title="用默认浏览器在地图中查看这个坐标"
        >
          在地图中查看
        </button>
      ) : undefined,
    },
    {
      // 地点行已经显示坐标时不必重复；只有显示城市名时才单列一行坐标
      label: '坐标',
      value: place && gps ? gpsText(gps) : null,
      wrap: true,
    },
    { label: '快门', value: exif?.exposureTime ?? null },
    { label: '光圈', value: exif?.fNumber ?? null },
    { label: 'ISO', value: exif?.iso ? `ISO ${exif.iso}` : null },
    { label: '焦距', value: exif ? focalText(exif) : null },
    { label: '曝光补偿', value: exif?.exposureBias ?? null },
    { label: '曝光程序', value: exif?.exposureProgram ?? null },
    { label: '测光模式', value: exif?.meteringMode ?? null },
    { label: '闪光灯', value: exif?.flash ?? null },
    { label: '白平衡', value: exif?.whiteBalance ?? null },
    { label: '方向', value: exif?.orientation ?? null },
    { label: '软件', value: exif?.software ?? null },
  ]
  // 无值的行为空，过滤掉（"拍摄地点"没 GPS 时整行消失，不留空行）
  const rows = allRows.filter((row) => row.value !== null && row.value !== '')

  return (
    <aside className="pv-info" aria-label="图片信息">
      <div className="pv-info-head">
        <span className="pv-info-title">图片信息</span>
        <button className="pv-info-close" onClick={onClose} aria-label="关闭图片信息" title="关闭">
          ×
        </button>
      </div>
      <dl className="pv-info-list">
        {rows.map(({ label, value, action, wrap }) => (
          <div className={wrap ? 'pv-info-row pv-info-row-wrap' : 'pv-info-row'} key={label}>
            <dt>{label}</dt>
            <dd>
              <span title={String(value)}>{value}</span>
              {action}
              {label === '拍摄地点' && mapError && (
                <span className="pv-info-map-error">{mapError}</span>
              )}
            </dd>
          </div>
        ))}
      </dl>
    </aside>
  )
}

/**
 * 错误解析与诊断信息生成。
 *
 * **为什么要这个**：错误信息要能被用户复制出来反馈给开发者，
 * 截图转述既慢又容易丢关键信息（错误码、堆栈、时间）。
 *
 * 支持四类输入的归一化：后端 ErrorDto / JS Error / 字符串 / 未知值。
 */

import type { DecodeError, ErrorDetailInfo } from './types'

/** 把后端错误转成用户可读的中文提示 */
export function toUserMessage(err: unknown): string {
  const d = decodeError(err, '')
  return d.userVisible ? d.message : '读取文件时发生错误'
}

/** 把任意错误对象解析成结构化诊断信息 */
export function decodeError(err: unknown, fallbackMessage: string): ErrorDetailInfo {
  const time = new Date().toISOString()

  // 情况 1：后端返回的 ErrorDto
  if (err && typeof err === 'object' && 'code' in err && 'userVisible' in err) {
    const e = err as DecodeError
    const parts = [
      '===== PurePhotoExplorer 错误诊断信息 =====',
      `时间: ${time}`,
      `错误码: ${e.code}`,
      `提示: ${e.message}`,
      `是否用户可见: ${e.userVisible ? '是' : '否'}`,
    ]
    return {
      code: e.code,
      source: 'Rust 后端',
      path: null,
      time,
      message: e.message,
      userVisible: e.userVisible,
      technical: '',
      full: parts.join('\n'),
    }
  }

  // 情况 2：JS 异常（Error 对象）
  if (err instanceof Error) {
    const detail = [
      `消息: ${err.message}`,
      `名称: ${err.name}`,
      err.stack ? `堆栈:\n${err.stack}` : '',
    ]
      .filter(Boolean)
      .join('\n')

    return {
      code: 'js_error',
      source: '前端',
      path: null,
      time,
      message: fallbackMessage || err.message,
      userVisible: true,
      technical: detail,
      full: [
        '===== PurePhotoExplorer 错误诊断信息 =====',
        `时间: ${time}`,
        '错误码: js_error',
        `消息: ${err.message}`,
        '',
        detail,
      ].join('\n'),
    }
  }

  // 情况 3：字符串
  if (typeof err === 'string') {
    return {
      code: 'unknown',
      source: '未知',
      path: null,
      time,
      message: err || fallbackMessage || '未知错误',
      userVisible: true,
      technical: '',
      full: `===== PurePhotoExplorer 错误诊断信息 =====\n时间: ${time}\n消息: ${err}`,
    }
  }

  // 情况 4：完全未知
  const msg = fallbackMessage || '未知错误'
  return {
    code: 'unknown',
    source: '未知',
    path: null,
    time,
    message: msg,
    userVisible: true,
    technical: err ? safeStringify(err) : '',
    full: [
      '===== PurePhotoExplorer 错误诊断信息 =====',
      `时间: ${time}`,
      `消息: ${msg}`,
      err ? `原始值: ${safeStringify(err)}` : '',
    ]
      .filter(Boolean)
      .join('\n'),
  }
}

/** 安全序列化（循环引用不炸） */
function safeStringify(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2)
  } catch {
    return String(v)
  }
}

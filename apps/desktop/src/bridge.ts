import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { AutostartStatus, ConnectionState, HistoryEntry, PublicSettings, ServerInfo, SettingsInput, TransferEvent } from './types'

export const native = isTauri()
export interface FileUploadPrompt { id: string; bytes: number; thresholdMb: number }
const noNative = () => Promise.reject(new Error('请在 EasyCopy 桌面客户端中使用此功能。'))

export const api = {
  previewImage: (id: string): Promise<ArrayBuffer> => native ? invoke('preview_image', { id }) : noNative(),
  history: (): Promise<HistoryEntry[]> => native ? invoke('list_history') : Promise.resolve([]),
  settings: (): Promise<PublicSettings> => native ? invoke('get_settings') : Promise.resolve({ address: '', port: 8443, hasPairingKey: false }),
  autostart: (): Promise<AutostartStatus> => native ? invoke('get_autostart') : noNative(),
  setAutostart: (enabled: boolean): Promise<AutostartStatus> => native ? invoke('set_autostart', { enabled }) : noNative(),
  connection: (): Promise<ConnectionState> => native ? invoke('get_connection') : Promise.resolve({ connected: false, message: '浏览器预览 · 未连接' }),
  test: (input: SettingsInput): Promise<ServerInfo> => native ? invoke('test_connection', { input }) : noNative(),
  save: (input: SettingsInput): Promise<void> => native ? invoke('save_settings', { input }) : noNative(),
  copy: (id: string): Promise<void> => native ? invoke('copy_item', { id }) : noNative(),
  cancelTransfer: (id: string): Promise<void> => native ? invoke('cancel_transfer', { id }) : noNative(),
  retryDownload: (id: string): Promise<void> => native ? invoke('retry_download', { id }) : noNative(),
  saveItem: (id: string): Promise<void> => native ? invoke('save_item', { id }) : noNative(),
  fileUploadPrompt: (): Promise<FileUploadPrompt | null> => native ? invoke('get_file_upload_prompt') : Promise.resolve(null),
  fileUploadThreshold: (): Promise<number> => native ? invoke('get_file_upload_threshold') : Promise.resolve(50),
  setFileUploadThreshold: (mb: number): Promise<void> => native ? invoke('set_file_upload_threshold', { mb }) : noNative(),
  resolveFileUpload: (id: string, upload: boolean): Promise<void> => native ? invoke('resolve_file_upload', { id, upload }) : noNative(),
  overlayReady: (): Promise<void> => native ? invoke('overlay_ready') : Promise.resolve(),
  overlayHidden: (taskId: string): Promise<void> => native ? invoke('overlay_hidden', { taskId }) : Promise.resolve(),
}

export async function events(handlers: {
  history?: (items: HistoryEntry[]) => void
  connection?: (state: ConnectionState) => void
  transfer?: (event: TransferEvent) => void
  fileUpload?: (prompt: FileUploadPrompt | null) => void
}): Promise<() => void> {
  if (!native) return () => {}
  const removers: (() => void)[] = []
  try {
    if (handlers.history) removers.push(await listen<HistoryEntry[]>('history-changed', event => handlers.history?.(event.payload)))
    if (handlers.connection) removers.push(await listen<ConnectionState>('connection-changed', event => handlers.connection?.(event.payload)))
    if (handlers.transfer) removers.push(await listen<TransferEvent>('transfer', event => handlers.transfer?.(event.payload)))
    if (handlers.fileUpload) removers.push(await listen<FileUploadPrompt | null>('file-upload-prompt', event => handlers.fileUpload?.(event.payload)))
    return () => removers.forEach(remove => remove())
  } catch (error) {
    removers.forEach(remove => remove())
    throw error
  }
}

export type ContentKind = 'text' | 'image' | 'files'

export interface HistoryEntry {
  id: string
  kind: ContentKind
  deviceName: string
  createdAt: number
  sequence: number | null
  preview: string
  detail: string
  thumbnail: string | null
  status: string
  available: boolean
  canCancel: boolean
  canRetryDownload: boolean
  error: string | null
}

export interface PublicSettings {
  address: string
  port: number
  hasPairingKey: boolean
}

export interface AutostartStatus { enabled: boolean; canEnable: boolean; reason: string | null }

export interface SettingsInput {
  address: string
  port: number
  pairingKey: string
}

export interface ConnectionState { connected: boolean; message: string }
export interface ServerInfo { protocol_version: number; server_id: string; logical_name: string }

export interface TransferEvent {
  taskId: string
  direction: 'upload' | 'download'
  phase: 'start' | 'preparing' | 'transferring' | 'verifying' | 'complete' | 'failed' | 'cancelled'
  receivedBytes: number
  totalBytes: number | null
  error: string | null
}

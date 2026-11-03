import type { TransferEvent } from './types'
export interface TransferState extends TransferEvent {
  progress: number
  opacity: number
  elapsedMs: number
  finished: boolean
  fadeStartMs: number | null
}
export class TransferIndicator {
  constructor(options: {
    draw: (state: TransferState) => void
    onHidden?: (taskId: string) => void
    minimumVisibleMs?: number
    fadeMs?: number
    now?: () => number
    requestFrame?: (callback: (time: number) => void) => number
    cancelFrame?: (id: number) => void
  })
  accept(event: TransferEvent): boolean
  setReducedMotion(reduced: boolean): void
  destroy(): void
}

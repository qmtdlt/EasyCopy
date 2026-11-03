// Real transfer lifecycle. Time never manufactures byte progress.
export class TransferIndicator {
  constructor({ draw, onHidden = () => {}, minimumVisibleMs = 300, fadeMs = 140,
    now = () => performance.now(), requestFrame = callback => requestAnimationFrame(callback),
    cancelFrame = id => cancelAnimationFrame(id) }) {
    Object.assign(this, { draw, onHidden, minimumVisibleMs, fadeMs, now, requestFrame, cancelFrame })
    this.generation = 0
    this.frameId = null
    this.current = null
    this.retired = new Set()
  }

  accept(event) {
    if (!event.taskId || this.retired.has(event.taskId)) return false
    if (this.current?.taskId !== event.taskId) {
      // An old task's progress/completion must never replace the current task.
      if (this.current && event.phase !== 'start') return false
      if (this.current) this.retired.add(this.current.taskId)
      this.stopFrame()
      this.current = { taskId: event.taskId, direction: event.direction, phase: event.phase,
        progress: 0, receivedBytes: 0, totalBytes: null, firstFrame: null, terminalAt: null,
        terminal: false, error: null }
    }
    const task = this.current
    if (task.terminal) return false
    // A duplicate start is idempotent and cannot reset visible time or bytes.
    if (event.phase !== 'start' || task.firstFrame === null) task.phase = event.phase
    if (event.phase !== 'start') {
      task.receivedBytes = Math.max(0, Number(event.receivedBytes) || 0)
      task.totalBytes = event.totalBytes === null ? null : Math.max(0, Number(event.totalBytes) || 0)
      task.progress = task.totalBytes > 0 ? Math.min(1, task.receivedBytes / task.totalBytes) : 0
    }
    if (event.phase === 'complete' || event.phase === 'failed') {
      task.terminal = true
      task.terminalAt = task.firstFrame === null ? null : this.now()
      task.error = event.error || null
      if (event.phase === 'complete') task.progress = 1
    }
    this.schedule()
    return true
  }

  schedule() {
    if (this.frameId !== null || !this.current) return
    const generation = this.generation
    this.frameId = this.requestFrame(timestamp => {
      if (generation !== this.generation || !this.current) return
      this.frameId = null
      const task = this.current
      task.firstFrame ??= timestamp
      if (task.terminal) task.terminalAt ??= timestamp
      const elapsedMs = Math.max(0, timestamp - task.firstFrame)
      const fadeStart = task.terminal ? Math.max(task.firstFrame + this.minimumVisibleMs, task.terminalAt) : null
      const fadeRatio = fadeStart === null || timestamp < fadeStart ? 0 : this.fadeMs === 0 ? 1 : Math.min(1, (timestamp - fadeStart) / this.fadeMs)
      const finished = fadeRatio >= 1
      this.draw({ ...task, elapsedMs, opacity: 1 - fadeRatio * fadeRatio * (3 - 2 * fadeRatio), finished,
        fadeStartMs: fadeStart === null ? null : fadeStart - task.firstFrame })
      if (generation !== this.generation) return
      if (finished) {
        this.retired.add(task.taskId)
        this.current = null
        this.onHidden(task.taskId)
      } else if (task.terminal) this.schedule()
    })
  }

  stopFrame() {
    this.generation++
    if (this.frameId !== null) this.cancelFrame(this.frameId)
    this.frameId = null
  }

  setReducedMotion(reduced) { this.fadeMs = reduced ? 0 : 140 }

  destroy() { this.stopFrame(); this.current = null; this.retired.clear() }
}

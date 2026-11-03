export class TransferIndicator {
  constructor({ draw, onState = () => {}, minimumVisibleMs = 300, fadeMs = 140,
    requestFrame = (callback) => requestAnimationFrame(callback),
    cancelFrame = (id) => cancelAnimationFrame(id) }) {
    Object.assign(this, { draw, onState, minimumVisibleMs, fadeMs, requestFrame, cancelFrame });
    this.generation = 0;
    this.frameId = null;
  }

  emit(state) {
    this.state = state;
    this.draw(state);
    this.onState(state);
  }

  stop() {
    this.generation += 1;
    if (this.frameId !== null) this.cancelFrame(this.frameId);
    this.frameId = null;
  }

  freeze(progress = 0.5) {
    this.stop();
    this.emit({ phase: 'preview', progress: Math.max(0, Math.min(1, progress)), opacity: 1,
      elapsedMs: 0, completionMs: null, fadeStartMs: null, finished: false });
  }

  play(durationMs = 1200) {
    this.stop();
    const generation = this.generation;
    const duration = Math.max(0, Number(durationMs) || 0);
    let firstFrame = null;
    let completionMs = null;

    const tick = (timestamp) => {
      if (generation !== this.generation) return;
      // Count from the first rendered frame, not from a click or background timer.
      firstFrame ??= timestamp;
      const elapsedMs = Math.max(0, timestamp - firstFrame);
      const progress = duration === 0 ? 1 : Math.min(1, elapsedMs / duration);
      if (progress === 1) completionMs ??= elapsedMs;
      const fadeStartMs = completionMs === null ? null : Math.max(this.minimumVisibleMs, completionMs);
      const isFading = fadeStartMs !== null && elapsedMs >= fadeStartMs;
      const fadeRatio = isFading ? (this.fadeMs === 0 ? 1 : Math.min(1, (elapsedMs - fadeStartMs) / this.fadeMs)) : 0;
      const eased = fadeRatio * fadeRatio * (3 - 2 * fadeRatio);
      const finished = fadeRatio >= 1;
      const phase = finished ? 'hidden' : isFading ? 'fading' : progress === 1 ? 'holding' : 'transferring';
      this.emit({ phase, progress, opacity: 1 - eased, elapsedMs, completionMs, fadeStartMs, finished });
      this.frameId = finished ? null : this.requestFrame(tick);
    };
    this.frameId = this.requestFrame(tick);
  }
}

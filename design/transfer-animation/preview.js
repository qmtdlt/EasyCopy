(async () => {
  const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const fadeMs = reducedMotion ? 0 : 140;
  const sprite = document.querySelector('#sprite');
  const cards = [...document.querySelectorAll('.demo')];
  const loopToggle = document.querySelector('#loop');
  const scrub = document.querySelector('#scrub');
  const timers = new Map();
  let durationMs = 1200;

  const smooth = (low, high, value) => {
    const t = Math.max(0, Math.min(1, (value - low) / (high - low)));
    return t * t * (3 - 2 * t);
  };

  // Render the supplied raster artwork. Only the white front-paper material is
  // tinted; page shape, highlights, folded corner and arrow come from the PNG.
  function makeFrames(index, fillColor) {
    const w = Math.floor(sprite.naturalWidth / 2), h = sprite.naturalHeight;
    const base = document.createElement('canvas');
    base.width = w; base.height = h;
    const ctx = base.getContext('2d', { willReadFrequently: true });
    ctx.drawImage(sprite, index * w, 0, w, h, 0, 0, w, h);
    const pixels = ctx.getImageData(0, 0, w, h);
    const filled = new ImageData(new Uint8ClampedArray(pixels.data), w, h);
    const rowCounts = new Uint32Array(h), colCounts = new Uint32Array(w), faceRows = new Uint32Array(h);
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        const i = (y * w + x) * 4;
        const [r, g, b, a] = pixels.data.subarray(i, i + 4);
        if (a > 220) { rowCounts[y]++; colCounts[x]++; }
        const paper = smooth(.68, .9, r / Math.max(b, 1)) * smooth(.78, .94, g / Math.max(b, 1)) * smooth(90, 190, r);
        if (paper > .8 && a > 220) faceRows[y]++;
        if (paper > 0) {
          // This color layer is clipped to the reported progress at draw time.
          const light = Math.max(.85, Math.min(1.08, (r + g + b) / 750));
          filled.data[i] = r * (1 - paper) + fillColor[0] * light * paper;
          filled.data[i + 1] = g * (1 - paper) + fillColor[1] * light * paper;
          filled.data[i + 2] = b * (1 - paper) + fillColor[2] * light * paper;
        }
      }
    }
    const first = (array, threshold) => array.findIndex((n) => n > threshold);
    const last = (array, threshold) => array.findLastIndex((n) => n > threshold);
    const x = Math.max(0, first(colCounts, 18) - 6), y = Math.max(0, first(rowCounts, 18) - 6);
    const right = Math.min(w, last(colCounts, 18) + 7), bottom = Math.min(h, last(rowCounts, 18) + 7);
    const faceTop = first(faceRows, 50), faceBottom = last(faceRows, 50) + 1;
    const colorLayer = document.createElement('canvas');
    colorLayer.width = w; colorLayer.height = h;
    colorLayer.getContext('2d').putImageData(filled, 0, 0);
    return { base, colorLayer, x, y, width: right - x, height: bottom - y, faceTop, faceBottom };
  }

  try {
    const reference = document.querySelector('#reference');
    await Promise.all([sprite.decode(), reference.decode()]);
    // Sample the approved image inside its flat mint region, using the browser's
    // color conversion so the animated layer matches the reference PNG.
    const paletteCanvas = document.createElement('canvas');
    paletteCanvas.width = reference.naturalWidth; paletteCanvas.height = reference.naturalHeight;
    const paletteContext = paletteCanvas.getContext('2d', { willReadFrequently: true });
    paletteContext.drawImage(reference, 0, 0);
    const fillColor = paletteContext.getImageData(Math.round(reference.naturalWidth * .215), Math.round(reference.naturalHeight * .64), 1, 1).data;
    const indicators = cards.map((card, index) => {
      const frame = makeFrames(index, fillColor);
      const fillsDownward = card.dataset.direction === 'download';
      const canvas = card.querySelector('canvas');
      canvas.width = frame.width; canvas.height = frame.height;
      const ctx = canvas.getContext('2d');
      const status = card.querySelector('.status'), timing = card.querySelector('.timing');
      const phaseLabels = { preview: '定格预览', transferring: '传输中', holding: '完成 · 停留中', fading: '完成 · 淡出中', hidden: '已消失' };
      const indicator = new TransferIndicator({
        fadeMs,
        draw(state) {
          ctx.clearRect(0, 0, canvas.width, canvas.height);
          ctx.drawImage(frame.base, frame.x, frame.y, frame.width, frame.height, 0, 0, frame.width, frame.height);
          if (state.progress > 0) {
            const fillHeight = state.progress * (frame.faceBottom - frame.faceTop);
            const fillEdge = (fillsDownward ? frame.faceTop + fillHeight : frame.faceBottom - fillHeight) - frame.y;
            ctx.save();
            if (state.progress < 1) {
              ctx.beginPath();
              ctx.rect(0, fillsDownward ? 0 : fillEdge, canvas.width, fillsDownward ? fillEdge : canvas.height - fillEdge);
              ctx.clip();
            }
            ctx.drawImage(frame.colorLayer, frame.x, frame.y, frame.width, frame.height, 0, 0, frame.width, frame.height);
            ctx.restore();
          }
          canvas.style.opacity = state.opacity.toFixed(4);
          canvas.setAttribute('aria-hidden', String(state.finished));
          card.dataset.progress = state.progress.toFixed(4);
          card.dataset.opacity = state.opacity.toFixed(4);
          card.dataset.elapsed = state.elapsedMs.toFixed(2);
          card.dataset.completion = state.completionMs?.toFixed(2) ?? '';
          card.dataset.fadeStart = state.fadeStartMs?.toFixed(2) ?? '';
        },
        onState(state) {
          if (card.dataset.phase !== state.phase) {
            card.dataset.phase = state.phase;
            status.textContent = phaseLabels[state.phase];
          }
          if (state.phase === 'preview') timing.textContent = '';
          else if (state.finished) timing.textContent = `${Math.round(state.fadeStartMs)} ms 后淡出 · 已结束`;
          else if (state.phase === 'holding') timing.textContent = '保持到 300 ms';
          else timing.textContent = `${Math.round(state.elapsedMs)} ms`;
          if (state.finished && loopToggle.checked) {
            timers.set(index, setTimeout(() => { timers.delete(index); indicator.play(durationMs); }, 850));
          }
        },
      });
      indicator.freeze(.5);
      return indicator;
    });

    function cancelRepeat(index) {
      if (timers.has(index)) { clearTimeout(timers.get(index)); timers.delete(index); }
    }
    function play(index) { cancelRepeat(index); indicators[index].play(durationMs); }
    function playBoth() { indicators.forEach((_, index) => play(index)); }
    function freezeBoth() {
      loopToggle.checked = false;
      indicators.forEach((indicator, index) => { cancelRepeat(index); indicator.freeze(Number(scrub.value) / 100); });
    }
    function explain() {
      const text = durationMs === 0
        ? `瞬间到达完成状态，保持至 300 ms，再用 ${fadeMs} ms 淡出。`
        : `传输 ${durationMs / 1000} 秒，完成后用 ${fadeMs} ms 淡出；不会再额外停留 300 ms。`;
      document.querySelector('#explanation').textContent = text;
    }
    document.querySelectorAll('[data-duration]').forEach((button) => {
      button.addEventListener('click', () => {
        durationMs = Number(button.dataset.duration);
        document.querySelectorAll('[data-duration]').forEach((other) => other.setAttribute('aria-pressed', String(other === button)));
        explain(); playBoth();
      });
    });
    document.querySelector('#play-both').addEventListener('click', playBoth);
    cards.forEach((card, index) => card.querySelector('.replay').addEventListener('click', () => play(index)));
    loopToggle.addEventListener('change', () => {
      if (loopToggle.checked) playBoth();
      else indicators.forEach((_, index) => cancelRepeat(index));
    });
    document.querySelector('#icon-size').addEventListener('change', (event) => {
      document.documentElement.style.setProperty('--icon-size', `${event.target.value}px`);
    });
    scrub.addEventListener('input', freezeBoth);
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) freezeBoth();
    });
    document.querySelectorAll('button[disabled]').forEach((button) => button.disabled = false);
    document.body.dataset.ready = 'true';
    explain();

    if (new URLSearchParams(location.search).has('qa')) {
      document.body.classList.add('qa-mode');
      const comparison = document.querySelector('#comparison');
      comparison.hidden = false;
      comparison.querySelector('h2').textContent = '左：配色与造型参考　／　右：对称方向填充（半填充）';
      comparison.append(document.querySelector('.showcase'));
    }
  } catch (error) {
    const box = document.querySelector('#error');
    box.hidden = false;
    box.textContent = `图标加载失败：${error.message}`;
    console.error(error);
  }
})();

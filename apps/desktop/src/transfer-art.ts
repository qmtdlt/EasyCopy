// Tint only the white front sheet in the approved PNG, preserving its artwork.
interface Frame {
  base: HTMLCanvasElement
  filled: HTMLCanvasElement
  x: number; y: number; width: number; height: number
  faceTop: number; faceBottom: number
}
const smooth = (low: number, high: number, value: number) => {
  const t = Math.max(0, Math.min(1, (value - low) / (high - low)))
  return t * t * (3 - 2 * t)
}

function makeFrame(sprite: HTMLImageElement, index: number, color: Uint8ClampedArray): Frame {
  const width = Math.floor(sprite.naturalWidth / 2), height = sprite.naturalHeight
  const base = document.createElement('canvas')
  base.width = width; base.height = height
  const context = base.getContext('2d', { willReadFrequently: true })!
  context.drawImage(sprite, index * width, 0, width, height, 0, 0, width, height)
  const pixels = context.getImageData(0, 0, width, height)
  const filledPixels = new ImageData(new Uint8ClampedArray(pixels.data), width, height)
  const rows = new Uint32Array(height), columns = new Uint32Array(width), faceRows = new Uint32Array(height)
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4
      const r = pixels.data[i]!, g = pixels.data[i + 1]!, b = pixels.data[i + 2]!, a = pixels.data[i + 3]!
      if (a > 220) { rows[y]++; columns[x]++ }
      const paper = smooth(.68, .9, r / Math.max(b, 1)) * smooth(.78, .94, g / Math.max(b, 1)) * smooth(90, 190, r)
      if (paper > .8 && a > 220) faceRows[y]++
      if (paper > 0) {
        const light = Math.max(.85, Math.min(1.08, (r + g + b) / 750))
        filledPixels.data[i] = r * (1 - paper) + color[0]! * light * paper
        filledPixels.data[i + 1] = g * (1 - paper) + color[1]! * light * paper
        filledPixels.data[i + 2] = b * (1 - paper) + color[2]! * light * paper
      }
    }
  }
  const first = (values: Uint32Array, n: number) => values.findIndex(value => value > n)
  const last = (values: Uint32Array, n: number) => {
    for (let i = values.length - 1; i >= 0; i--) if (values[i]! > n) return i
    return -1
  }
  const x = Math.max(0, first(columns, 18) - 6), y = Math.max(0, first(rows, 18) - 6)
  const right = Math.min(width, last(columns, 18) + 7), bottom = Math.min(height, last(rows, 18) + 7)
  const filled = document.createElement('canvas')
  filled.width = width; filled.height = height
  filled.getContext('2d')!.putImageData(filledPixels, 0, 0)
  return { base, filled, x, y, width: right - x, height: bottom - y, faceTop: first(faceRows, 50), faceBottom: last(faceRows, 50) + 1 }
}

export async function loadTransferArt() {
  const sprite = new Image(), reference = new Image()
  sprite.src = '/assets/transfer-icons.png'; reference.src = '/assets/approved-reference.png'
  await Promise.all([sprite.decode(), reference.decode()])
  const palette = document.createElement('canvas')
  palette.width = reference.naturalWidth; palette.height = reference.naturalHeight
  const paletteContext = palette.getContext('2d', { willReadFrequently: true })!
  paletteContext.drawImage(reference, 0, 0)
  const color = paletteContext.getImageData(Math.round(reference.naturalWidth * .215), Math.round(reference.naturalHeight * .64), 1, 1).data
  const frames = [makeFrame(sprite, 0, color), makeFrame(sprite, 1, color)]
  return (canvas: HTMLCanvasElement, direction: 'upload' | 'download', progress: number) => {
    const frame = frames[direction === 'upload' ? 0 : 1]!
    if (canvas.width !== frame.width || canvas.height !== frame.height) { canvas.width = frame.width; canvas.height = frame.height }
    const context = canvas.getContext('2d')!
    context.clearRect(0, 0, canvas.width, canvas.height)
    context.drawImage(frame.base, frame.x, frame.y, frame.width, frame.height, 0, 0, frame.width, frame.height)
    if (progress <= 0) return
    const down = direction === 'download', fillHeight = progress * (frame.faceBottom - frame.faceTop)
    const edge = (down ? frame.faceTop + fillHeight : frame.faceBottom - fillHeight) - frame.y
    context.save()
    if (progress < 1) {
      context.beginPath()
      context.rect(0, down ? 0 : edge, canvas.width, down ? edge : canvas.height - edge)
      context.clip()
    }
    context.drawImage(frame.filled, frame.x, frame.y, frame.width, frame.height, 0, 0, frame.width, frame.height)
    context.restore()
  }
}

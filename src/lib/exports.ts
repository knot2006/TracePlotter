// Saving and copying what the window shows: files through the backend's save dialog,
// graphs as images. The outcome is told in a toast.
import { showToast } from "./Toast.svelte";
import { exportFile } from "./trace";

/** Asks where to save the bytes, `name` being the suggestion; a cancelled dialog says nothing. */
export async function saveExport(name: string, data: Uint8Array) {
  try {
    const path = await exportFile(name, data);
    if (path) showToast(`Saved ${path}`);
  } catch (e) {
    showToast(`Export failed: ${e}`, true);
  }
}

/** Saves an image as a PNG. */
export function saveImage(image: HTMLCanvasElement, name: string) {
  image.toBlob(async (blob) => {
    if (blob) saveExport(name, new Uint8Array(await blob.arrayBuffer()));
  });
}

/** Puts an image on the clipboard as a PNG. */
export async function copyImage(image: HTMLCanvasElement, what: string) {
  try {
    // The item takes the blob as a promise so that the copy still counts as part of the click.
    const blob = new Promise<Blob>((resolve, reject) =>
      image.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("no image"))), "image/png"),
    );
    await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
    showToast(`${what} copied to the clipboard`);
  } catch (e) {
    showToast(`Copy failed: ${e}`, true);
  }
}

/** A line of text over a graph, in pieces of their own colour. */
export type Caption = { text: string; color: string }[];

/**
 * Graphs one under the other, each under its caption, on the app's background. Their
 * canvases are transparent, so on their own they would show whatever is behind them.
 */
export function captionedImage(parts: { canvas: HTMLCanvasElement; caption: Caption }[]) {
  if (!parts.length) return null;
  const captionHeight = 28 * devicePixelRatio;
  const image = document.createElement("canvas");
  image.width = Math.max(...parts.map((part) => part.canvas.width));
  image.height = parts.reduce((sum, part) => sum + part.canvas.height + captionHeight, 0);
  const ctx = image.getContext("2d")!;
  ctx.fillStyle = "#0f1319";
  ctx.fillRect(0, 0, image.width, image.height);
  ctx.font = `${13 * devicePixelRatio}px "Segoe UI", sans-serif`;
  ctx.textBaseline = "middle";
  let y = 0;
  for (const { canvas, caption } of parts) {
    let x = 12 * devicePixelRatio;
    for (const { text, color } of caption) {
      ctx.fillStyle = color;
      ctx.fillText(text, x, y + captionHeight / 2);
      x += ctx.measureText(text).width;
    }
    ctx.drawImage(canvas, 0, y + captionHeight);
    y += captionHeight + canvas.height;
  }
  return image;
}

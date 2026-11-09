import type { Worker } from "tesseract.js";

export interface OcrWord {
  text: string;
  confidence: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface OcrProgress {
  status: string;
  progress: number;
}

/** Load the OCR engine only when requested; language data is cached by Tesseract.js. */
export async function recognizeImage(
  image: HTMLCanvasElement,
  languages: string,
  onProgress: (progress: OcrProgress) => void,
): Promise<{ text: string; words: OcrWord[] }> {
  const { createWorker } = await import("tesseract.js");
  let worker: Worker | null = null;
  try {
    worker = await createWorker(languages, undefined, {
      logger: ({ status, progress }) => onProgress({ status, progress }),
    });
    const { data } = await worker.recognize(image, {}, { blocks: true });
    const words: OcrWord[] = [];
    for (const block of data.blocks ?? []) {
      for (const paragraph of block.paragraphs) {
        for (const line of paragraph.lines) {
          for (const word of line.words) {
            const { x0, y0, x1, y1 } = word.bbox;
            if (word.text.trim() && x1 > x0 && y1 > y0) {
              words.push({
                text: word.text,
                confidence: word.confidence,
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
              });
            }
          }
        }
      }
    }
    return { text: data.text, words };
  } finally {
    if (worker) await worker.terminate();
  }
}

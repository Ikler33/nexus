/**
 * Тип файла по расширению — для выбора просмотрщика vs текстового редактора (Ф4-10).
 * Картинки и PDF открываются вьюером (бинарь нельзя читать как текст), markdown/текст — в CM6.
 */
const IMAGE_EXT = /\.(png|jpe?g|gif|svg|webp|avif|bmp|ico)$/i;
const PDF_EXT = /\.pdf$/i;

export function isImage(path: string): boolean {
  return IMAGE_EXT.test(path);
}

export function isPdf(path: string): boolean {
  return PDF_EXT.test(path);
}

/** Открывается ли файл просмотрщиком (не текстовым редактором). */
export function isViewable(path: string): boolean {
  return isImage(path) || isPdf(path);
}

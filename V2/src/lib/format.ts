export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

export type FileTone = "red" | "blue" | "green" | "orange" | "violet" | "grey";

/** Short type label + colour for a file name, e.g. "PDF" / red. */
export function fileTag(name: string): { label: string; tone: FileTone } {
  const ext = (name.split(".").pop() ?? "").toLowerCase();
  if (ext === "pdf") return { label: "PDF", tone: "red" };
  if (["doc", "docx", "docm"].includes(ext)) return { label: "DOC", tone: "blue" };
  if (["xls", "xlsx", "xlsm", "ods", "csv", "tsv"].includes(ext)) return { label: "XLS", tone: "green" };
  if (["ppt", "pptx", "pptm"].includes(ext)) return { label: "PPT", tone: "orange" };
  if (["md", "markdown", "mdx"].includes(ext)) return { label: "MD", tone: "grey" };
  if (["txt", "rst", "log", "adoc", "org"].includes(ext)) return { label: "TXT", tone: "grey" };
  if (["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp"].includes(ext)) return { label: "IMG", tone: "orange" };
  if (!ext || ext === name.toLowerCase()) return { label: "FILE", tone: "grey" };
  return { label: ext.slice(0, 4).toUpperCase(), tone: "violet" };
}

export function basename(p: string): string {
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

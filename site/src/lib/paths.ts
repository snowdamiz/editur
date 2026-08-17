export function withBase(path: string): string {
  const base = import.meta.env.BASE_URL;
  const prefix = base.endsWith("/") ? base : `${base}/`;
  return `${prefix}${path.replace(/^\//, "")}`;
}

export function withBase(path: string): string {
  const base = import.meta.env.BASE_URL;
  const prefix = base.endsWith("/") ? base : `${base}/`;
  return `${prefix}${path.replace(/^\//, "")}`;
}

export function absoluteUrl(path: string, site: URL | undefined): string {
  const origin = site ?? new URL("https://snowdamiz.github.io");
  return new URL(path.startsWith("/") ? path : withBase(path), origin).href;
}

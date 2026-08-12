export const RELEASE_DOWNLOAD =
  "https://github.com/snowdamiz/editur/releases/download/release";

export type DownloadId = "macos-arm" | "macos-intel" | "linux" | "windows";

export type DownloadSpec = {
  id: DownloadId;
  href: string;
  label: string;
  detail: string;
  option: string;
};

export const DOWNLOADS: Record<DownloadId, DownloadSpec> = {
  "macos-arm": {
    id: "macos-arm",
    href: `${RELEASE_DOWNLOAD}/editur-macos-aarch64.zip`,
    label: "Download Editur.app",
    detail: "macOS · Apple Silicon",
    option: "Apple Silicon",
  },
  "macos-intel": {
    id: "macos-intel",
    href: `${RELEASE_DOWNLOAD}/editur-macos-x86_64.zip`,
    label: "Download Editur.app",
    detail: "macOS · Intel",
    option: "Intel",
  },
  linux: {
    id: "linux",
    href: `${RELEASE_DOWNLOAD}/editur-linux-x86_64`,
    label: "Download Editur",
    detail: "Linux · x86_64",
    option: "Linux",
  },
  windows: {
    id: "windows",
    href: `${RELEASE_DOWNLOAD}/editur-windows-x86_64.exe`,
    label: "Download Editur.exe",
    detail: "Windows · x86_64",
    option: "Windows",
  },
};

export const DOWNLOAD_ORDER: DownloadId[] = [
  "macos-arm",
  "macos-intel",
  "linux",
  "windows",
];

export function downloadIdFromNavigator(input: {
  platform: string;
  userAgent: string;
}): DownloadId {
  const { platform, userAgent } = input;
  if (/win/i.test(platform) || /Windows NT/i.test(userAgent)) {
    return "windows";
  }
  const linux =
    (/linux/i.test(platform) || /Linux|X11/i.test(userAgent)) &&
    !/Android/i.test(userAgent);
  if (linux) return "linux";
  return "macos-arm";
}

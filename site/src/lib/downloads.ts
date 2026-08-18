export const RELEASE_DOWNLOAD =
  "https://github.com/snowdamiz/editur/releases/download/release";

export type DownloadId = "macos-arm" | "macos-intel";

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
};

export const DOWNLOAD_ORDER: DownloadId[] = ["macos-arm", "macos-intel"];

import { WIN_INSTALL } from "../lib/commands";
import {
  DOWNLOADS,
  downloadIdFromNavigator,
  type DownloadId,
} from "../lib/downloads";

const root = document.documentElement;
const toggle = document.getElementById("theme-toggle");
const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function applyTheme(mode: string) {
  root.setAttribute("data-theme", mode);
  toggle?.setAttribute(
    "aria-label",
    mode === "dark" ? "Switch to light theme" : "Switch to dark theme",
  );
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute("content", mode === "dark" ? "#0c0c0e" : "#f6f6f8");
}

applyTheme(root.getAttribute("data-theme") || "dark");

toggle?.addEventListener("click", () => {
  const next = root.getAttribute("data-theme") === "dark" ? "light" : "dark";
  applyTheme(next);
  try {
    localStorage.setItem("editur-theme", next);
  } catch {
    /* private mode */
  }
});

const ua = navigator.userAgent;
const platform =
  (navigator as Navigator & { userAgentData?: { platform?: string } })
    .userAgentData?.platform || navigator.platform || "";
const isWindows = /win/i.test(platform) || /Windows/i.test(ua);
const isLinux =
  !isWindows &&
  (/linux/i.test(platform) || /Linux|X11/i.test(ua)) &&
  !/Android/i.test(ua);
const renderer = isWindows ? "d3d12" : isLinux ? "vulkan" : "metal";

document
  .querySelector(`[data-renderer="${renderer}"]`)
  ?.setAttribute("data-host", "true");

const installCmd = document.getElementById("cta-cmd") ?? document.getElementById("hero-cmd");
if (isWindows && installCmd) {
  installCmd.textContent = WIN_INSTALL;
  installCmd.dataset.command = WIN_INSTALL;
}

function applyDownload(id: DownloadId) {
  const spec = DOWNLOADS[id];
  document.querySelectorAll<HTMLAnchorElement>("[data-download]").forEach((el) => {
    el.href = spec.href;
  });
  document.querySelectorAll("[data-download-label]").forEach((el) => {
    el.textContent = spec.label;
  });
  document.querySelectorAll("[data-download-detail]").forEach((el) => {
    el.textContent = spec.detail;
  });
  document.querySelectorAll("[data-download-option]").forEach((el) => {
    const on = el.getAttribute("data-download-option") === id;
    if (on) el.setAttribute("aria-current", "true");
    else el.removeAttribute("aria-current");
  });
}

applyDownload(
  downloadIdFromNavigator({
    platform,
    userAgent: ua,
  }),
);

document.querySelectorAll("[data-download-option]").forEach((el) => {
  el.addEventListener("click", (event) => {
    event.preventDefault();
    const id = el.getAttribute("data-download-option");
    if (id && id in DOWNLOADS) applyDownload(id as DownloadId);
  });
});

if (isWindows || isLinux) {
  document.querySelectorAll(".mod").forEach((node) => {
    node.textContent = "Ctrl";
  });
}

const tabs = [
  document.getElementById("tab-unix"),
  document.getElementById("tab-win"),
].filter((tab): tab is HTMLElement => Boolean(tab));

function selectTab(active: HTMLElement) {
  for (const tab of tabs) {
    const on = tab === active;
    tab.setAttribute("aria-selected", String(on));
    const panel = document.getElementById(tab.getAttribute("aria-controls") || "");
    if (panel instanceof HTMLElement) panel.hidden = !on;
  }
}

tabs.forEach((tab, index) => {
  tab.addEventListener("click", () => selectTab(tab));
  tab.addEventListener("keydown", (event) => {
    const step =
      event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    event.preventDefault();
    const next = tabs[(index + step + tabs.length) % tabs.length];
    if (!next) return;
    selectTab(next);
    next.focus();
  });
});

if (isWindows && tabs[1]) selectTab(tabs[1]);

document.querySelectorAll<HTMLButtonElement>("[data-copy]").forEach((button) => {
  const label = button.querySelector("span");
  const original = label?.textContent ?? "Copy";
  let timer = 0;

  button.addEventListener("click", () => {
    const target = document.getElementById(button.dataset.copy || "");
    const text = (
      target?.dataset.command ||
      target?.textContent ||
      ""
    ).trim();
    if (!text) return;

    const done = () => {
      if (label) label.textContent = "Copied";
      button.setAttribute("data-copied", "true");
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (label) label.textContent = original;
        button.removeAttribute("data-copied");
      }, 1600);
    };

    const fallback = () => {
      const field = document.createElement("textarea");
      field.value = text;
      field.setAttribute("readonly", "");
      field.style.position = "fixed";
      field.style.opacity = "0";
      document.body.appendChild(field);
      field.select();
      try {
        document.execCommand("copy");
        done();
      } catch {
        /* ignore */
      }
      document.body.removeChild(field);
    };

    if (navigator.clipboard?.writeText) {
      navigator.clipboard.writeText(text).then(done, fallback);
    } else {
      fallback();
    }
  });
});

const pending = document.querySelectorAll(".rise:not(.in)");
if (!reduced && "IntersectionObserver" in window) {
  const reveal = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        entry.target.classList.add("in");
        reveal.unobserve(entry.target);
      }
    },
    { rootMargin: "0px 0px -12% 0px", threshold: 0 },
  );
  pending.forEach((node) => reveal.observe(node));
  window.setTimeout(() => {
    document.querySelectorAll(".rise:not(.in)").forEach((node) => {
      node.classList.add("in");
    });
  }, 1400);
} else {
  pending.forEach((node) => node.classList.add("in"));
}

const navLinks = Array.from(
  document.querySelectorAll<HTMLAnchorElement>("a[data-spy]"),
);
const sections = navLinks
  .map((link) => document.querySelector(link.getAttribute("href") || ""))
  .filter((node): node is HTMLElement => node instanceof HTMLElement);

function currentSection() {
  const probe = window.innerHeight * 0.32;
  let current: HTMLElement | undefined;
  for (const section of sections) {
    if (section.getBoundingClientRect().top <= probe) current = section;
  }
  return current?.id;
}

function syncSpy() {
  const id = currentSection();
  if (!id) return;
  for (const link of navLinks) {
    const on = link.getAttribute("href") === `#${id}`;
    if (on) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
}

let spyFrame = 0;
function onScroll() {
  if (spyFrame) return;
  spyFrame = requestAnimationFrame(() => {
    spyFrame = 0;
    syncSpy();
  });
}

if (sections.length) {
  window.addEventListener("scroll", onScroll, { passive: true });
  window.addEventListener("resize", onScroll);
  syncSpy();
}

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const dist = join(dirname(fileURLToPath(import.meta.url)), "..", "dist");
const html = readFileSync(join(dist, "index.html"), "utf8");

test("ships the verified macOS install command", () => {
  assert.match(
    html,
    /curl --proto '=https' --tlsv1\.2 --retry 5 --retry-all-errors -LsSf https:\/\/raw\.githubusercontent\.com\/snowdamiz\/editur\/release\/install\.sh \| sh/,
  );
  assert.doesNotMatch(html, /install\.ps1/);
});

test("document is named, skippable, and branded", () => {
  assert.match(html, /<title>Editur/);
  assert.match(html, /Skip to content/);
  assert.match(html, /rel="icon"/);
  assert.match(html, /og:image/);
});

test("points at the continuous release and the open command", () => {
  assert.match(html, /github\.com\/snowdamiz\/editur/);
  assert.match(html, /editur \./);
});

test("logo and og image URLs keep the /editur base segment", () => {
  assert.doesNotMatch(html, /\/editureditur/);
  assert.doesNotMatch(html, /snowdamiz\.github\.io\/og\.png/);
  assert.match(html, /src="[^"]*editur-icon[^"]*"/);
  assert.match(html, /og:image" content="https:\/\/snowdamiz\.github\.io\/editur\//);
});

test("the header is the only place the mark appears", () => {
  const logos = html.match(/src="[^"]*editur-icon[^"]*"/g) ?? [];
  assert.equal(logos.length, 1);
});

test("does not ship byte-identical PNG assets twice", () => {
  const hashes = readdirSync(dist, { recursive: true })
    .filter((path) => path.endsWith(".png"))
    .map((path) =>
      createHash("sha256").update(readFileSync(join(dist, path))).digest("hex"),
    );
  assert.equal(new Set(hashes).size, hashes.length);
});

test("ships only the used WOFF2 font faces", () => {
  const fonts = readdirSync(join(dist, "_astro")).filter((path) => path.includes("woff"));
  assert.ok(fonts.every((path) => path.endsWith(".woff2")), fonts);
  assert.ok(!fonts.some((path) => path.includes("serif-latin-400-normal")), fonts);
});

test("install commands truncate instead of scrolling sideways", () => {
  assert.match(html, /data-command="[^"]+" class="[^"]*\btruncate\b/);
});

test("offers both macOS downloads and no unsupported release targets", () => {
  assert.match(
    html,
    /https:\/\/github\.com\/snowdamiz\/editur\/releases\/download\/release\/editur-macos-aarch64\.zip/,
  );
  assert.match(
    html,
    /https:\/\/github\.com\/snowdamiz\/editur\/releases\/download\/release\/editur-macos-x86_64\.zip/,
  );
  assert.doesNotMatch(html, /editur-(?:linux|windows)/);
  assert.match(html, /Download Editur\.app/);
  assert.match(html, /Install from the terminal/);
});

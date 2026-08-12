import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const html = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "..", "dist", "index.html"),
  "utf8",
);

test("ships both verified install commands", () => {
  assert.match(
    html,
    /curl --proto '=https' --tlsv1\.2 -LsSf https:\/\/raw\.githubusercontent\.com\/snowdamiz\/editur\/release\/install\.sh \| sh/,
  );
  assert.match(
    html,
    /irm https:\/\/raw\.githubusercontent\.com\/snowdamiz\/editur\/release\/install\.ps1 \| iex/,
  );
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

test("install commands truncate instead of scrolling sideways", () => {
  assert.match(html, /data-command="[^"]+" class="[^"]*\btruncate\b/);
});

test("offers a native download for each release target and a terminal installer", () => {
  assert.match(
    html,
    /https:\/\/github\.com\/snowdamiz\/editur\/releases\/download\/release\/editur-macos-aarch64\.zip/,
  );
  assert.match(
    html,
    /https:\/\/github\.com\/snowdamiz\/editur\/releases\/download\/release\/editur-macos-x86_64\.zip/,
  );
  assert.match(
    html,
    /https:\/\/github\.com\/snowdamiz\/editur\/releases\/download\/release\/editur-linux-x86_64"/,
  );
  assert.match(
    html,
    /https:\/\/github\.com\/snowdamiz\/editur\/releases\/download\/release\/editur-windows-x86_64\.exe/,
  );
  assert.match(html, /Download Editur\.app/);
  assert.match(html, /Download Editur\.exe/);
  assert.match(html, /Install from the terminal/);
});

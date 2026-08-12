import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const css = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "..", "src", "styles", "global.css"),
  "utf8",
);

function themeBlock(selector) {
  const start = css.indexOf(selector);
  assert.ok(start >= 0, `missing ${selector}`);
  const open = css.indexOf("{", start);
  const close = css.indexOf("}", open);
  return css.slice(open + 1, close);
}

function token(block, name) {
  const match = block.match(new RegExp(`${name}:\\s*(#[0-9a-fA-F]+)`));
  assert.ok(match, `missing ${name}`);
  return match[1].toLowerCase();
}

test("light theme is paper, not a gray well with white cards", () => {
  const light = themeBlock('[data-theme="light"]');
  const sunken = token(light, "--color-sunken");
  const raised = token(light, "--color-raised");

  assert.notEqual(sunken, "#d2d2d7", "page must not use the editor's gray well");
  assert.notEqual(raised, "#ffffff", "cards must not lift to white over a gray page");
  assert.match(sunken, /^#f[0-9a-f]{5}$/, "page should be paper");
  assert.equal(raised, sunken, "cards sit on the page, distinguished by hairlines");
});

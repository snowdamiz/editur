import assert from "node:assert/strict";
import { test } from "node:test";
import { DOWNLOADS, DOWNLOAD_ORDER } from "../src/lib/downloads.ts";

test("offers only the two macOS release builds", () => {
  assert.deepEqual(DOWNLOAD_ORDER, ["macos-arm", "macos-intel"]);
  assert.deepEqual(Object.keys(DOWNLOADS), DOWNLOAD_ORDER);
});

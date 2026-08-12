import assert from "node:assert/strict";
import { test } from "node:test";
import { downloadIdFromNavigator } from "../src/lib/downloads.ts";

test("picks a native asset from the platform string", () => {
  assert.equal(
    downloadIdFromNavigator({ platform: "Win32", userAgent: "Mozilla/5.0 (Windows NT 10.0)" }),
    "windows",
  );
  assert.equal(
    downloadIdFromNavigator({ platform: "Linux x86_64", userAgent: "Mozilla/5.0 (X11; Linux x86_64)" }),
    "linux",
  );
  assert.equal(
    downloadIdFromNavigator({ platform: "MacIntel", userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)" }),
    "macos-arm",
  );
});

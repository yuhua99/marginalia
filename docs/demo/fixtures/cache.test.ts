import { expect, test } from "vitest";
import { loadFeed } from "../src/feed/cache";
import { simulateFeed } from "./fixtures";

test("keeps the last good feed when offline", async () => {
  const lastGoodFeed = [{ id: "dawn", title: "Dawn in Taipei" }];
  simulateFeed({ cached: lastGoodFeed, network: "offline", stale: true });
  expect(await loadFeed()).toEqual(lastGoodFeed);
});

test("surfaces errors when no cached feed is available", async () => {
  simulateFeed({ network: "offline" });
  await expect(loadFeed()).rejects.toThrow("offline");
});

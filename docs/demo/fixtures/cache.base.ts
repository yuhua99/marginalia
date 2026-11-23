import { readCache, writeCache } from "./storage";
import { fetchPhotos } from "./api";
import type { Photo } from "./types";

const TTL = 60_000;

export async function loadFeed(): Promise<Photo[]> {
  const cached = await readCache();
  if (cached && Date.now() - cached.savedAt < TTL) {
    return cached.photos;
  }

  const photos = await fetchPhotos();
  const savedAt = Date.now();
  await writeCache({ photos, savedAt });
  return photos;
}

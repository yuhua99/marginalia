#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --workspace
python3 docs/demo/seed.py
vhs docs/demo/discuss.tape
vhs docs/demo/picker.tape
filter='split[a][b];[a]crop=1512:696:0:0[top];[b]crop=1512:40:0:728[bottom];[top][bottom]vstack=inputs=2'
for image in hero picker; do
  ffmpeg -hide_banner -loglevel error -y \
    -i "target/readme-demo/$image-source.png" \
    -filter_complex "$filter" -frames:v 1 -threads 1 \
    "docs/assets/$image.png"
done
ffmpeg -hide_banner -loglevel error -y \
  -i target/readme-demo/discuss.mp4 \
  -filter_complex_threads 2 -filter_complex "fps=16,$filter" \
  -an -c:v libwebp_anim -quality 90 -compression_level 6 -loop 0 \
  docs/assets/discuss.webp

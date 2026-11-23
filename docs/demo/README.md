# README captures

Run `bash docs/demo/render.sh` from a checkout. Requirements: Cargo, Git,
Python 3, VHS 0.12.1, ttyd 1.7.7, FFmpeg with libwebp, a Chromium browser
supported by VHS, and JetBrains Mono installed.

The script builds mrg and replaces only the disposable fixture repository at
`target/readme-demo/fieldnotes`. It uses an isolated mrg config, fixed commit
dates, no remotes, and no real review data. Fixtures are illustrative TypeScript,
not a runnable application. No forge credentials are needed.

Both tapes use 1512 × 768, JetBrains Mono 18px, and Catppuccin Mocha.
The render step removes the native bottom key-hint row (32 pixels), preserving
the frame below it. Final assets are 1512 × 736. There are no added labels,
captions, or key overlays. The discussion has a 2.3-second opening hold,
human-paced typing, and a 3.9-second closing hold; the picker is a still only.

VHS writes intermediate captures under `target/readme-demo/`. Only the final
PNG/WebP files under `docs/assets/` belong in git. Theme/font/rendering versions
can affect pixel output; commit IDs are stable but mrg session IDs are generated.

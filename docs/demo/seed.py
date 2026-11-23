#!/usr/bin/env python3
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
WORK = ROOT / "target/readme-demo"
REPO = WORK / "fieldnotes"
TARGET = Path(json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--no-deps"],
    cwd=ROOT, text=True,
))["target_directory"])
MRG = TARGET / "debug/mrg"
ENV = dict(os.environ, XDG_CONFIG_HOME=str(WORK / "config"),
           GIT_AUTHOR_NAME="Maya Chen", GIT_AUTHOR_EMAIL="maya@example.com",
           GIT_COMMITTER_NAME="Maya Chen", GIT_COMMITTER_EMAIL="maya@example.com",
           GIT_AUTHOR_DATE="2026-01-15T10:00:00Z",
           GIT_COMMITTER_DATE="2026-01-15T10:00:00Z")


def run(*args, **kwargs):
    return subprocess.check_output(args, cwd=REPO, env=ENV, text=True, **kwargs).strip()


def commit(message):
    run("git", "add", "--", "src", "tests")
    run("git", "-c", "commit.gpgsign=false", "commit", "-qm", message)


mode = sys.argv[1] if len(sys.argv) > 1 else "seed"
if mode in ("review", "picker"):
    os.chdir(REPO)
    args = [str(MRG)]
    if mode == "review":
        args += ["resume", "--session", (WORK / "session-id").read_text().strip()]
    os.execve(str(MRG), args, ENV)
if mode != "seed":
    raise SystemExit("usage: seed.py [seed|review|picker]")
if REPO.exists():
    shutil.rmtree(REPO)
REPO.mkdir(parents=True)
config = WORK / "config/marginalia"
config.mkdir(parents=True, exist_ok=True)
(config / "settings.toml").write_text('theme = "catppuccin"\n')
run("git", "init", "--quiet", "--initial-branch=main")
(REPO / "src/feed").mkdir(parents=True)
(REPO / "tests").mkdir()
fixtures = ROOT / "docs/demo/fixtures"
shutil.copy(fixtures / "cache.base.ts", REPO / "src/feed/cache.ts")
commit("Cache photo feed responses")
run("git", "tag", "before-offline")
shutil.copy(fixtures / "cache.next.ts", REPO / "src/feed/cache.ts")
commit("Keep a cached feed visible offline")
cache_commit = run("git", "rev-parse", "HEAD")
shutil.copy(fixtures / "cache.test.ts", REPO / "tests/cache.test.ts")
commit("Test offline fallback and empty-cache errors")
run("git", "tag", "offline-feed")
session = run(str(MRG), "new", "before-offline..offline-feed",
              "--name", "Keep the photo feed readable offline")
(WORK / "session-id").write_text(session + "\n")
comments = [
    {"body": "Keep the last good feed visible when the network drops."},
    {"commit": cache_commit, "file": "src/feed/cache.ts", "lines": "19-21",
     "body": "Only network failures use the cache. Server errors still surface."},
]
run(str(MRG), "comment", "--session", session, "-", input=json.dumps(comments))
run("git", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm",
    "Update the contributor guide")
print("Seeded offline-feed review and commit picker in " + str(REPO))

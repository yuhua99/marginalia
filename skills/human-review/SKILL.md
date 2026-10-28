---
name: human-review
description: "Walk a human through a code change in marginalia (mrg)."
disable-model-invocation: true
---

# Human Review

Run every command from the repo root; sessions belong to the repo.

## Tool
`mrg` (`mrg <cmd> --help` for flags):
- `mrg new RANGE [--name NAME]` prints a session id. NAME: the change's purpose, at most 32 columns. RANGE is `A..END`; END is a commit, `HEAD`, `staged`, or `unstaged` (working tree). Working changes: `HEAD..unstaged`. Write `HEAD` rather than its sha so fix commits join the same session.
- `mrg comment --session ID FILE` (`-` for stdin) posts `{"from": "agent", "comments": [...]}`. Each comment has `body` and exactly one of `reply_to: "<thread id>"` or `anchor`:
  - `{"kind": "session"}`, `{"kind": "commit", "commit": REV}`, `{"kind": "file", "file": PATH}`
  - `{"kind": "lines", "file": PATH, "start": N, "end": M}` anchors new-side lines at the range end. Add `"rev": BASE` (`base` in `mrg show`) for deleted lines.
- `mrg show --session ID` prints the session as JSON; `--only waiting` keeps the threads waiting on you, `--only outdated` those whose lines changed since.
- `mrg delete --session ID THREAD...` deletes your notes: threads the user never wrote in.
- `{baseDir}/scripts/review-tab ID` opens the session in a review tab and reports back here when the human quits.

Ask once if the scope is missing.

## Workflow
1. Read the scoped `git show/diff` and surrounding code.
   Act as the change's author walking a human through the implementation, rather than conducting a separate defect review.
   Done when you can explain the old and new behavior of each meaningful changed block.

2. Group hunks by concern and draft all comments into a temp comments file.
   Split each hunk into logical blocks of one or a few adjacent lines. Explain each block's old behavior -> change -> new behavior, and why; explain intent and effects rather than paraphrasing syntax.
   Give distinct changes within a function separate comments, anchored to the relevant lines.
   Summarize tests, codegen, and pure moves in one line per logical group; still name any behavior or test-expectation changes. For copied blocks, link the source in one line; explain any differences from the source and their behavioral effects separately.
   Cite code outside the diff (a caller, a definition, a copy source) as a link: a repo-relative `path:line` or `path:start-end` at the range end.
   Ground reasons in the implementation context; label inferred reasons as assumptions.
   Add a likely objection or toy before/after only when it clarifies a trade-off or a confusing change. Open a comment on a known problem with `問題：`. Redact secrets.
   Done when every changed hunk is covered by block-level explanations or explicitly included in a brief-summary exception above.

3. Order the file, `mrg new RANGE --name NAME`, `mrg comment` it, then `review-tab ID`:
   - Session anchor first: the change's purpose, which concern to read first, and why. Add one ASCII sketch of the change's shape: a call tree, file tree, or pseudocode with `+`/`-` lines, holding only the nodes this change touches.
   - Lines anchors: the block-level explanations and brief summaries from step 2.
   - File anchors: only context not already covered by the session or lines comments.

4. Follow-up:
   - `waiting on agent` → `mrg show --only waiting`; for each thread, answer or fix, then reply in it with `reply_to`. After fixing, `delete` your notes (threads with no `user` message) in `mrg show --only outdated` that the fix made wrong. Post replies and comments on any new delta, then `review-tab` the same ID.

## Gotchas
- History rewritten below the base → `mrg resume` fails in the review tab and nothing arrives; `mrg new` a new session.
- Abnormal exit sends nothing; run `mrg show` when the human says done.

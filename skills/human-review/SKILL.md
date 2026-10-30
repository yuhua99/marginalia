---
name: human-review
description: "Walk a human through a code change in marginalia (mrg)."
disable-model-invocation: true
---

# Human Review

Run every command from the repo root; sessions belong to the repo.

## Tool
`mrg` (`mrg <cmd> --help` for flags):
- `mrg new RANGE [--name NAME]` prints a session id. NAME: the change's purpose, at most 32 columns. RANGE is `A..END`; END is a commit, `HEAD`, `staged`, or `unstaged` (working tree, tracked files only). Working changes: `HEAD..unstaged`. Write `HEAD` rather than its sha so fix commits join the same session.
- `mrg diff RANGE [PATH...]` prints the diff with each line's old and new number, as `comment` takes them; read it in place of `git diff`.
- `mrg comment --session ID FILE` (`-` for stdin) posts a JSON array of comments. Each has `body` and, for what it is on:
  - `"reply_to": "<thread id>"` answers in that thread.
  - `"commit": REV`, a commit of the range.
  - `"file": PATH`; add `"lines": "N"` or `"N-M"` for lines of it, numbered at the range end. Add `"old": true` for lines the range deletes, numbered at the base; a file the end has not is the base's.
  - None of them: the session.
- `mrg show --session ID` prints the session as JSON; `--only waiting` keeps the threads waiting on you, `--only outdated` those whose lines changed since. A thread's `location` is where its lines are now; its `anchor` keeps where they were when written.
- `mrg delete --session ID THREAD...` deletes your notes: threads the user never wrote in.
- `{baseDir}/scripts/review-tab ID` opens the session in a review tab and reports back here when the human quits; outside herdr/tmux it prints a command to relay to the human.

Ask once if the scope is missing.
A range over 5 commits: split it along feature boundaries into consecutive sub-ranges of at most 5 commits, one session each, and run the workflow per session, one at a time.

## Workflow
1. Read the scope with `mrg diff RANGE`, its commit messages with `git log`, and surrounding code.
   Act as the change's author walking a human through the implementation, rather than conducting a separate defect review.
   Done when you can explain the old and new behavior of each meaningful changed block.

2. Group hunks by concern and draft all comments into a temp comments file.
   Split each hunk into logical blocks of one or a few adjacent lines.
   Keep each comment to one point a reader takes in at a glance, at most three short lines; split rather than lengthen.
   Explain each block's old behavior -> change -> new behavior, and why; explain intent and effects rather than paraphrasing syntax.
   Give distinct changes within a function separate comments, anchored to the relevant lines.
   Summarize tests, codegen, and pure moves in one line per logical group; still name any behavior or test-expectation changes. For copied blocks, link the source in one line; explain any differences from the source and their behavioral effects separately.
   Cite code outside the diff (a caller, a definition, a copy source) as a link: a repo-relative `path:line` or `path:start-end` at the range end.
   Ground reasons in the implementation context; label inferred reasons as assumptions.
   Add a likely objection or toy before/after only when it clarifies a trade-off or a confusing change. Open a comment on a known problem with `問題：`. Redact secrets.
   Done when every changed hunk is covered by block-level explanations or explicitly included in a brief-summary exception above.

3. Order the file, `mrg new RANGE --name NAME`, `mrg comment` it, then `review-tab ID`:
   - Session anchor first: the change's purpose, which concern to read first, and why.
   - Commit anchors: one per commit in the range, a line on what the commit does.
   - Lines anchors: the block-level explanations and brief summaries from step 2.
   - File anchors: only context not already covered by the session or lines comments.

4. Follow-up, once the human is back (a `waiting on agent` notice, pasted threads, or done):
   - `mrg show --only waiting`; nothing waiting ends the review.
   - For each thread, answer or fix, then reply in it with `reply_to`.
   - After fixing, `delete` your notes (threads with no `user` message) in `mrg show --only outdated` that the fix made wrong.
   - Read the new delta with `mrg diff BASE..END`. BASE is the `base` sha of `mrg show`; a relative base like `HEAD~2` moves with fix commits.
   - Post replies and comments on the delta, then `review-tab` the same ID.

## Gotchas
- History rewritten below the base → `mrg resume` fails in the review tab and nothing arrives; `mrg new` a new session.

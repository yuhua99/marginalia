<h1 align="center">marginalia</h1>

<p align="center"><em>Review agent-written code, right where it changed.</em></p>

<img src="docs/assets/hero.png" width="100%" alt="Marginalia reviewing an offline photo-feed change: two commits, a file tree, and a line-anchored conversation between the agent and reviewer." />

## Code review, as a conversation

**Discuss a change in context.**<br>
Ask the agent about the exact lines you’re reviewing.

<img src="docs/assets/discuss.webp" width="100%" alt="A reviewer replies to the agent’s explanation of an offline cache fallback; the thread stays beside the changed lines and switches to waiting on agent." />

**Review just the commits that belong together.**<br>
Select related commits and review their changes as one diff.

<img src="docs/assets/picker.png" width="100%" alt="Marginalia’s commit picker with the offline-feed implementation and test commits selected, leaving the unrelated contributor-guide commit unselected." />

## Install

```sh
git clone https://github.com/yuhua99/marginalia.git
cd marginalia
cargo install --path crates/mrg
```

Install the skill:

```sh
npx skills add yuhua99/marginalia
```

## Usage

In your agent:

```
/human-review review latest 3 commits
```

Works best inside herdr/rex/tmux: the review opens in a new tab and the agent picks up when you quit. Elsewhere, the agent prints a command for you to run.

Run `mrg` to pick an open pull / merge request to review; needs [`gh`](https://cli.github.com) or [`glab`](https://gitlab.com/gitlab-org/cli).

## Acknowledgements

Inspired by [tuicr](https://github.com/agavra/tuicr).

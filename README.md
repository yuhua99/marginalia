<h1 align="center">marginalia</h1>

<p align="center"><em>Review code changes in the terminal and discuss them with agents.</em></p>

## Install

```sh
git clone https://github.com/yuhua99/marginalia.git
cd marginalia
cargo install --path .
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

Works best inside herdr or tmux: the review opens in a new tab and the agent picks up when you quit. Elsewhere, the agent prints a command for you to run.

## Acknowledgements

Inspired by [tuicr](https://github.com/agavra/tuicr).

## Gix Of Theseus

A re-implementation of Erik Bern's [Git Of Theseus](https://github.com/erikbern/git-of-theseus), with fewer features but hundreds of times faster.

Generates cohort analysis graphs of git repositories over time:
[[img here]]

It' fast because it uses a specialized algorithm (inspired from [hercules](https://github.com/src-d/hercules)) to implement its own "incremental" git blame, and because it's written in Rust, which gives it access to the wonderful [gitoxide](https://github.com/GitoxideLabs/gitoxide) and [rayon](https://docs.rs/rayon/latest/rayon/) crates.

## Installation

Installation will be with `cargo install gix-of-theseus` once I publish the package.

For now, clone this repository and run `cargo build --release` to compile it. Then run `cargo install --path .`

`uv` is recommended to be able to run the plotting scripts "automagically".

## Usage

To get an image directly, (if you have `uv` installed):

```
gix-of-theseus ~/repos/git/git --image-file git.png
```

Will save its results to `${repo_name}.png`. Choose the output file's location with `--output-file`.
Omitting the `--plot` flag will collect the data in the same cohorts.json format but not plot it.

The plotting script from Git Of Theseus has been re-included in this repo and updated to the PEP 723 single script file standard, so you can run it with `uv` without needing pip install or a virtualenv:

```
uv run src/stackplot.py cohorts.json
# future
gix-of-theseus stackplot cohorts.json
```

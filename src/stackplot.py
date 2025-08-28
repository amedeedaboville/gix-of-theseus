#!/usr/bin/env uv python
# /// script
# requires-python = ">=3.8"
# dependencies = [
#     "matplotlib",
#     "numpy",
#     "python-dateutil",
# ]
# ///

# Copyright 2016 Erik Bernhardsson
# Copyright 2025 Amédée d'Aboville (modifications)
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
# http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.


import argparse
import dateutil.parser
import itertools
import json
import matplotlib
from matplotlib import pyplot
import numpy


def generate_n_colors(n: int) -> list[tuple[float, float, float]]:
    vs = numpy.linspace(0.4, 0.9, 6)
    colors = [(0.9, 0.4, 0.4)]

    def euclidean(a, b):
        return sum((x - y) ** 2 for x, y in zip(a, b))

    while len(colors) < n:
        new_color = max(
            itertools.product(vs, vs, vs),
            key=lambda a: min(euclidean(a, b) for b in colors),
        )
        colors.append(new_color)
    return colors

def stack_plot(
    input_fn: str,
    display: bool = False,
    outfile: str = "stack_plot.png",
    max_n: int = 20,
    normalize: bool = False,
) -> None:
    if not display:
        matplotlib.use("Agg")
    with open(input_fn) as f:
        data = json.load(f)
    y = numpy.array(data["y"])
    if y.shape[0] > max_n:
        js = sorted(range(len(data["labels"])), key=lambda j: max(y[j]), reverse=True)
        other_sum = y[js[max_n:]].sum(axis=0)
        top_js = sorted(js[:max_n], key=lambda j: data["labels"][j])
        y = numpy.array([y[j] for j in top_js] + [other_sum])
        labels = [data["labels"][j] for j in top_js] + ["other"]
    else:
        labels = data["labels"]
    if normalize:
        y = 100.0 * numpy.array(y) / numpy.sum(y, axis=0)
    pyplot.figure(figsize=(16, 12), dpi=120)
    pyplot.style.use("ggplot")
    ts = [dateutil.parser.parse(t) for t in data["ts"]]
    colors = generate_n_colors(len(labels))
    pyplot.stackplot(ts, y, labels=labels, colors=colors)
    pyplot.legend(loc=2)
    if normalize:
        pyplot.ylabel("Share of lines of code (%)")
        pyplot.ylim([0, 100])
    else:
        pyplot.ylabel("Lines of code")
    print(f"Writing output to {outfile}")
    pyplot.savefig(outfile)
    pyplot.tight_layout()
    if display:
        pyplot.show()


def stack_plot_cmdline() -> None:
    parser = argparse.ArgumentParser(description="Plot stack plot")
    parser.add_argument("--display", action="store_true", help="Display plot")
    parser.add_argument(
        "--outfile",
        default="stack_plot.png",
        type=str,
        help="Output file to store results (default: %(default)s)",
    )
    parser.add_argument(
        "--max-n",
        default=20,
        type=int,
        help='Max number of dataseries (will roll everything else into "other") (default: %(default)s)',
    )
    parser.add_argument(
        "--normalize", action="store_true", help="Normalize the plot to 100%%"
    )
    parser.add_argument("input_fn")
    kwargs = vars(parser.parse_args())

    stack_plot(**kwargs)


if __name__ == "__main__":
    stack_plot_cmdline()

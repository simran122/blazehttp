"""Builds the one-binary writer A/B workspace: engine-core at three revisions as renamed crates plus a bench binary.

Run from the repository root: python3 docs/perf/harness/writer-ab/build.py OUT_DIR BASE_REV FR1_REV NEW_REV
(journal 2026-11-13: 68c48f0 = pre-review, abe8c87 = fix round 1, the fix-round-2 head). OUT_DIR must be outside the
repository and must not contain it; it is created, or replaced only when an earlier run of this script made it (its
marker file is present). OUT_DIR/provenance.txt records each revision's commit.
"""

import io
import os
import re
import shutil
import subprocess
import sys
import tarfile

HERE = os.path.dirname(os.path.abspath(__file__))
VARIANTS = ("base", "fr1", "new")
# Written into OUT_DIR so that a later run replaces only a folder this script made.
MARKER = ".writer-ab-build"
PROVENANCE = "provenance.txt"
USAGE = "usage: python3 docs/perf/harness/writer-ab/build.py OUT_DIR BASE_REV FR1_REV NEW_REV (from the repo root)"
BENCH_MANIFEST = """[package]
name = "writer-ab"
version = "0.0.0"
edition.workspace = true
publish = false

[dependencies]
engine-core = { package = "engine-core-new", path = "../new" }
engine-core-base = { path = "../base" }
engine-core-fr1 = { path = "../fr1" }
"""


def git(*args: str) -> bytes:
    return subprocess.run(["git", *args], check=True, capture_output=True).stdout


def export(rev: str, dest: str) -> None:
    data = git("archive", rev, "crates/engine-core/src", "crates/engine-core/Cargo.toml",
               "crates/engine-core/tests/support")
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        tar.extractall(dest, filter="data")


def add_revision(out: str, name: str, rev: str) -> str:
    """Exports engine-core at `rev` as the crate `engine-core-<name>`; returns the commit `rev` names."""
    tmp = os.path.join(out, f"_{name}")
    export(rev, tmp)
    shutil.move(os.path.join(tmp, "crates", "engine-core"), os.path.join(out, name))
    shutil.rmtree(tmp)
    manifest = os.path.join(out, name, "Cargo.toml")
    with open(manifest) as source:
        text = source.read()
    text = text.replace('name = "engine-core"', f'name = "engine-core-{name}"')
    text = re.sub(r"\n# Linked by.*?(?=\n\[dev-dependencies\])", "", text, flags=re.S)
    text = text.split("[dev-dependencies]")[0]
    with open(manifest, "w") as target:
        target.write(text)
    return git("rev-parse", "--verify", f"{rev}^{{commit}}").decode().strip()


def refusal(out: str, repo: str) -> str | None:
    """Why OUT_DIR must not be (re)made, or None. Both paths are real (symlinks resolved)."""
    if os.path.commonpath([out, repo]) in (out, repo):
        return f"OUT_DIR {out} must be outside the repository {repo} and must not contain it"
    if os.path.lexists(out) and not os.path.isdir(out):
        return f"OUT_DIR {out} exists and is not a folder"
    if os.path.isdir(out) and os.listdir(out) and not os.path.isfile(os.path.join(out, MARKER)):
        return f"OUT_DIR {out} is not empty and was not made by this script (no {MARKER}); pick another folder"
    return None


def main(argv: list[str]) -> None:
    if len(argv) != 5:
        sys.exit(USAGE)
    repo = os.path.realpath(git("rev-parse", "--show-toplevel").decode().strip())
    if os.path.realpath(os.getcwd()) != repo:
        sys.exit(f"run from the repository root {repo}\n{USAGE}")
    out = os.path.realpath(argv[1])
    reason = refusal(out, repo)
    if reason is not None:
        sys.exit(reason)
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)
    with open(os.path.join(out, MARKER), "w") as marker:
        marker.write("made by docs/perf/harness/writer-ab/build.py; replaced by its next run\n")
    commits = {name: add_revision(out, name, rev) for name, rev in zip(VARIANTS, argv[2:5], strict=True)}
    with open(os.path.join(out, PROVENANCE), "w") as provenance:
        for name, rev in zip(VARIANTS, argv[2:5], strict=True):
            provenance.write(f"{name} {rev} {commits[name]}\n")
    src = os.path.join(out, "bench", "src")
    os.makedirs(src)
    for support in ("write_bench.rs", "write_rings.rs"):
        shutil.copy(os.path.join(out, "new", "tests", "support", support), os.path.join(src, support))
    shutil.copy(os.path.join(HERE, "main.rs"), os.path.join(src, "main.rs"))
    with open(os.path.join(out, "bench", "Cargo.toml"), "w") as manifest:
        manifest.write(BENCH_MANIFEST)
    with open("Cargo.toml") as source:
        workspace = source.read()
    workspace = workspace.replace('members = ["crates/*"]', 'members = ["base", "fr1", "new", "bench"]')
    workspace = workspace.replace('exclude = ["crates/engine-core/fuzz"]\n', "")
    with open(os.path.join(out, "Cargo.toml"), "w") as target:
        target.write(workspace)
    # The repository's lock pins the crates the three revisions share; cargo drops the entries they do not use.
    for name in ("rust-toolchain.toml", "Cargo.lock"):
        shutil.copy(name, os.path.join(out, name))
    with open(os.path.join(out, PROVENANCE)) as provenance:
        print(provenance.read(), end="")


if __name__ == "__main__":
    main(sys.argv)

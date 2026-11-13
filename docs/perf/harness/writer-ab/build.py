"""Builds the one-binary writer A/B workspace: engine-core at three revisions as renamed crates plus a bench binary.

Run from the repository root: python3 docs/perf/harness/writer-ab/build.py OUT_DIR BASE_REV FR1_REV NEW_REV
(journal 2026-11-13: 68c48f0 = pre-review, abe8c87 = fix round 1, the fix-round-2 head). OUT_DIR must be outside the
repository; it is replaced.
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


def add_revision(out: str, name: str, rev: str) -> None:
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
    print(name, git("rev-parse", rev).decode().strip())


def main() -> None:
    out = os.path.abspath(sys.argv[1])
    revs = dict(zip(VARIANTS, sys.argv[2:5], strict=True))
    if os.path.commonpath([out, os.getcwd()]) == os.getcwd():
        sys.exit("OUT_DIR must be outside the repository")
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)
    for name, rev in revs.items():
        add_revision(out, name, rev)
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
    shutil.copy("rust-toolchain.toml", os.path.join(out, "rust-toolchain.toml"))


main()

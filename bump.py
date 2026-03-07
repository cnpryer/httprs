import argparse
import re
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--version", required=True)
args = parser.parse_args()

v = args.version.lstrip("v")
cargo_v = re.sub(r"a(\d+)$", r"-alpha.\1", v)
cargo_v = re.sub(r"b(\d+)$", r"-beta.\1", cargo_v)
cargo_v = re.sub(r"rc(\d+)$", r"-rc.\1", cargo_v)

for path, new_ver in [("pyproject.toml", v), ("Cargo.toml", cargo_v)]:
    text = Path(path).read_text()
    text = re.sub(
        r'^version = ".*"', f'version = "{new_ver}"', text, count=1, flags=re.MULTILINE
    )
    Path(path).write_text(text)

print(f"Bumped to {v} (pyproject.toml) / {cargo_v} (Cargo.toml)")

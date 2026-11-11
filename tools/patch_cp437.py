"""Emit the cp437 table as an ASCII-only Rust array and patch it into repair.rs.

ASCII on purpose: a non-ASCII literal in a Rust file is legal but this project has
already been burned twice by tooling that re-encoded a source file on the way past.
"""
import pathlib
import re

root = pathlib.Path(__file__).resolve().parent.parent
src = root / "crates" / "core" / "src" / "repair.rs"

tbl = [bytes([i]).decode("cp437") for i in range(256)]
rows = []
for i in range(0, 256, 16):
    row = ", ".join("'\\u{%04X}'" % ord(c) for c in tbl[i:i + 16])
    rows.append("    " + row + ",")
body = "\n".join(rows)
literal = "static CP437: [char; 256] = [\n%s\n];" % body

text = src.read_text(encoding="utf-8")
pattern = re.compile(r"^static CP437: &str = \".*\";$", re.M)
assert pattern.search(text), "CP437 string literal not found"
text = pattern.sub(lambda m: literal, text, count=1)
src.write_text(text, encoding="utf-8")

check = src.read_text(encoding="utf-8")
print("array present:", "static CP437: [char; 256] = [" in check)
print("non-ascii bytes in file:", sum(1 for b in check.encode("utf-8") if b > 0x7F))

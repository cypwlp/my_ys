"""Generate the cp437 table for crates/core/src/repair.rs and prove the round-trip.

Written as a file, not a shell heredoc: the shell re-encodes non-ASCII literals and
silently corrupts them, which has already bitten this project twice.
"""
import pathlib

tbl = "".join(bytes([i]).decode("cp437") for i in range(256))

parts = []
for c in tbl:
    o = ord(c)
    if o == 0x20:
        parts.append(" ")
    elif o < 0x20 or c in ('\\', '"'):
        parts.append("\\u{%04X}" % o)
    else:
        parts.append(c)
esc = "".join(parts)

out = pathlib.Path(".cp437.rust.txt")
out.write_text(esc, encoding="utf-8")

# The console codepage here is GBK, so non-ASCII never goes to stdout: it would
# raise UnicodeEncodeError before proving anything.
lines = ["table chars: %d  escaped bytes: %d" % (len(tbl), len(esc))]
lines.append("0x1F=%r 0x7F=%r 0x80=%r 0xC4=%r 0xFF=%r" % (tbl[0x1F], tbl[0x7F], tbl[0x80], tbl[0xC4], tbl[0xFF]))
for label in ("gb18030", "big5"):
    raw = "報告表2026/季報.xlsx".encode(label)
    moji = raw.decode("cp437")
    back = moji.encode("cp437")
    lines.append("%s byte-exact round trip: %s  re-decoded: %s  moji: %s"
                 % (label, back == raw, back.decode(label), moji))
pathlib.Path(".cp437.proof.txt").write_text("\n".join(lines), encoding="utf-8")
print(lines[0])
print("round trip gb18030:", "True" in lines[2].split("  ")[1])
print("round trip big5:", "True" in lines[3].split("  ")[1])
print("proof written to .cp437.proof.txt")

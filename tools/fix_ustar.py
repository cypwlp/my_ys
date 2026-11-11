"""Rewrite the ustar helper in the Rust test file.

The previous pass wrote real NUL bytes into the source instead of the two-character
escapes Rust needs, because the replacement text travelled through a python string
literal.  Here every backslash is built with chr(92), so nothing can be eaten.
"""
import pathlib

p = pathlib.Path("crates/core/tests/core.rs")
lines = p.read_text(encoding="utf-8").split(chr(10))

start = next(i for i, l in enumerate(lines) if l.startswith("fn ustar("))
# the helper's closing brace is the first bare "}" at column 0 after start
end = next(i for i in range(start, len(lines)) if lines[i] == "}")

new = r'''fn ustar(members: &[(&str, &[u8])]) -> Vec<u8> {
    fn octal(value: u64, width: usize) -> Vec<u8> {
        let mut v = format!("{value:0>width$o}").into_bytes();
        v.push(0x00);
        v
    }
    let mut out = Vec::new();
    for (name, data) in members {
        let mut block = vec![0u8; 512];
        block[0..name.len()].copy_from_slice(name.as_bytes());
        block[100..108].copy_from_slice(&octal(0o644, 7)); // mode
        block[108..116].copy_from_slice(&octal(0, 7)); // uid
        block[116..124].copy_from_slice(&octal(0, 7)); // gid
        block[124..136].copy_from_slice(&octal(data.len() as u64, 11)); // size
        block[136..148].copy_from_slice(&octal(1_700_000_000, 11)); // mtime
        block[156] = b'0'; // typeflag: regular file
        block[257..262].copy_from_slice(b"ustar");
        block[263..265].copy_from_slice(b"00");
        let sum: u64 = block.iter().map(|b| *b as u64).sum();
        block[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
        block[154] = 0x00;
        block[155] = b' ';
        out.extend_from_slice(&block);
        out.extend_from_slice(data);
        let pad = (512 - data.len() % 512) % 512;
        out.extend(std::iter::repeat(0u8).take(pad));
    }
    out.extend(std::iter::repeat(0u8).take(1024)); // two zero blocks end the archive
    out
}'''

body = new.split(chr(10))
lines[start:end + 1] = body
text = chr(10).join(lines)
p.write_text(text, encoding="utf-8")
print("NUL bytes remaining in file:", text.count(chr(0)))
print("ustar helper lines:", len(body))

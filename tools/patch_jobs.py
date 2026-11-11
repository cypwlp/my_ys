"""Stop a refused job from burning an id, and keep the settings sandbox env-free.

Kept as a file rather than a shell heredoc: the patch text contains quotes and
backslashes that the shell has already mangled once in this project.
"""
import pathlib

p = pathlib.Path("crates/core/src/jobs.rs")
t = p.read_text(encoding="utf-8")

old = """        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let token = CancelToken::new();
        let mut inner = self.inner.write().ok()?;
        if inner.running >= self.parallel {
            return None;
        }
"""
new = """        let mut inner = self.inner.write().ok()?;
        if inner.running >= self.parallel {
            // A refused job gets no id.  Ids are what the UI shows and cancels,
            // so burning one per rejection would make the numbering jump.
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let token = CancelToken::new();
"""
assert t.count(old) == 1, t.count(old)
p.write_text(t.replace(old, new), encoding="utf-8")
print("jobs.rs patched")

//! A 7z-shaped front door for the core, so behaviour can be measured from a shell
//! without starting a window.  Same verbs, same switch spellings, fewer surprises.

use qarc_core::{Algo, CompressOptions, ExtractRequest, Ops, Overwrite};
use std::io::IsTerminal;
use std::path::Path;

fn usage() -> ! {
    eprintln!(
        "qarc <command> [switches] [archive] [members...]

  l  x  t   list / extract / test          a   create or add
  h         sha256 or md5 of a file

  -o<dir>   destination        -p<pw>    password
  -t<fmt>   force format       -mx<n>    level 1..9
  -y        assume yes (default here)    -aos/-aot/-aou  skip / overwrite / auto-rename"
    );
    std::process::exit(2);
}

/// `-o` and `-p` are glued to their value in 7-Zip; `-t` is too.  Everything else
/// that starts with `-` and is unknown is an error rather than a silent member.
fn parse(args: &[String]) -> (String, ExtractRequest, CompressOptions, Vec<String>) {
    let mut req = ExtractRequest { overwrite: Overwrite::KeepBoth, repair_names: true, ..Default::default() };
    let mut opts = CompressOptions { level: 5, threads: 0, format: Default::default(), ..Default::default() };
    let mut positional = Vec::new();
    let mut command = String::new();
    for (i, token) in args.iter().enumerate() {
        if i == 0 {
            command = token.clone();
            continue;
        }
        if let Some(v) = token.strip_prefix("-o") {
            req.dest = v.to_string();
        } else if let Some(v) = token.strip_prefix("-p") {
            req.password = Some(v.to_string());
            opts.password = Some(v.to_string());
        } else if let Some(v) = token.strip_prefix("-t") {
            opts.format = match v.to_lowercase().as_str() {
                "7z" => qarc_core::Format::SevenZip,
                "zip" => qarc_core::Format::Zip,
                "tar" => qarc_core::Format::Tar,
                "tgz" | "tar.gz" => qarc_core::Format::TarGz,
                other => bad_switch(other),
            };
        } else if let Some(v) = token.strip_prefix("-mx") {
            opts.level = v.parse().unwrap_or(5);
        } else if token == "-aos" {
            req.overwrite = Overwrite::Skip;
        } else if token == "-aoa" {
            req.overwrite = Overwrite::Always;
        } else if token == "-aou" {
            req.overwrite = Overwrite::KeepBoth;
        } else if token == "-y" || token.starts_with("-scc") {
            // accepted, already the default
        } else if token.starts_with('-') {
            bad_switch(token);
        } else {
            positional.push(token.clone());
        }
    }
    (command, req, opts, positional)
}

fn bad_switch(token: &str) -> ! {
    eprintln!("不認得的開關：{token}");
    usage()
}

/// A single updating line on stderr, so stdout stays clean for piping and a long
/// archive is not a blank screen.  Throttled to ~10 Hz; padded, not ANSI, so it
/// works in a plain conhost too.  Redirection keeps stderr completely quiet.
struct ProgressLine {
    last: Option<std::time::Instant>,
    printed: bool,
    enabled: bool,
}

impl ProgressLine {
    fn new() -> Self {
        Self {
            last: None,
            printed: false,
            enabled: std::io::stderr().is_terminal(),
        }
    }

    fn report(&mut self, p: qarc_core::Progress) {
        if !self.enabled {
            return;
        }
        let now = std::time::Instant::now();
        if self
            .last
            .is_some_and(|last| now.duration_since(last).as_millis() < 100)
        {
            return;
        }
        self.last = Some(now);
        let head = match p.fraction {
            Some(f) => format!("{:3.0}%", (f * 100.0).clamp(0.0, 100.0)),
            None => qarc_core::human::size(p.done),
        };
        let mut chars: Vec<char> = p.summary.chars().collect();
        let tail = if chars.len() > 60 {
            format!("…{}", chars.split_off(chars.len() - 59).into_iter().collect::<String>())
        } else {
            p.summary
        };
        eprint!("\r{:<72}", format!("{head}  {tail}"));
        self.printed = true;
    }

    fn finish(&self) {
        if self.printed {
            eprintln!();
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let (command, mut req, opts, positional) = parse(&args);
    let ops = Ops::discover();
    let ids = ops.engine_ids().join(", ");

    match command.as_str() {
        "l" | "x" | "t" => {
            let Some(archive) = positional.first() else { usage() };
            req.archive = archive.clone();
            req.members = positional[1..].to_vec();
            req.test_only = command == "t";
            if command == "l" {
                let info = ops
                    .list(Path::new(archive), req.password.as_deref())
                    .unwrap_or_else(|e| fail(&e));
                println!(
                    "引擎 {} · 格式 {} · 共 {} 項",
                    info.engine, info.format, info.entries.len()
                );
                for e in info.entries {
                    println!(
                        "{:>10} {:>10} {:<20} {}{}",
                        qarc_core::human::size(e.size),
                        e.packed.map(qarc_core::human::size).unwrap_or_else(|| "-".into()),
                        e.mtime.map(qarc_core::human::time).unwrap_or_else(|| "-".into()),
                        e.path,
                        if e.unsafe_path { "  [阻擋]" } else { "" }
                    );
                }
            } else {
                if req.dest.is_empty() {
                    req.dest = ".".into();
                }
                let mut prog = ProgressLine::new();
                let done = ops
                    .extract(&req, &mut |p| prog.report(p), None)
                    .unwrap_or_else(|e| fail(&e));
                prog.finish();
                println!(
                    "{} 項 · {} · 阻擋 {} 項 · 引擎 {}",
                    done.items,
                    qarc_core::human::size(done.bytes_out),
                    done.blocked.len(),
                    done.engine
                );
                for b in done.blocked {
                    println!("  阻擋 {b}");
                }
                for e in done.errors {
                    println!("  錯誤 {e}");
                }
            }
        }
        "a" => {
            let Some(archive) = positional.first() else { usage() };
            let mut o = opts;
            o.archive = archive.clone();
            o.sources = positional[1..].to_vec();
            if o.sources.is_empty() {
                eprintln!("沒有來源檔案");
                std::process::exit(2);
            }
            if o.format == qarc_core::Format::Unknown {
                o.format = qarc_core::formats::from_extension(archive);
            }
            let mut prog = ProgressLine::new();
            let done = ops.create(&o, &mut |p| prog.report(p), None).unwrap_or_else(|e| fail(&e));
            prog.finish();
            println!(
                "{} 項 · {} → {} · 引擎 {} · 可用引擎 [{ids}]",
                done.items,
                qarc_core::human::size(done.bytes_out),
                o.archive,
                done.engine
            );
        }
        "h" => {
            let Some(path) = positional.first() else { usage() };
            println!("{}", qarc_core::digest_file(Path::new(path), Algo::Sha256).unwrap_or_else(|e| fail(&e)));
        }
        "engines" => {
            for row in ops.fleet_report() {
                println!("{} [{}] {}", row.id, row.kind, row.path.unwrap_or_else(|| "內建".into()));
                for (label, write) in row.formats {
                    println!("   {label:<8} 讀  {}寫", if write { "可" } else { "不可" });
                }
            }
        }
        other => {
            eprintln!("不認識的命令：{other}");
            usage();
        }
    }
}

fn fail<T>(e: &dyn std::fmt::Display) -> T {
    eprintln!("失敗：{e}");
    std::process::exit(1)
}

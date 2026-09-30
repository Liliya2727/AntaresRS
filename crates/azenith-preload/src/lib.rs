// Copyright (C) 2026-2027 Zexshia
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Page-cache preloader — a port of the vendored vmtouch 1.4.1 in
//! `preloadbin/jni/main.c`.
//!
//! ## What it actually does
//!
//! It walks a directory, `mmap`s each regular file `PROT_READ`, and (with `-t`)
//! reads one byte per page. That read is the whole mechanism: it is a demand
//! fault, so the kernel maps the page in. After a game's libraries are in the
//! page cache, launching it does not have to fault them in from flash.
//!
//! ## What was ported, and what was not
//!
//! The C is a general-purpose vmtouch with 18 options, a pidfile, daemon mode,
//! `mlock`, and NUL-delimited batch input. `GamePreload.c` invoked exactly one
//! form of it:
//!
//! ```text
//! sys.azenith-preloadbin -v -t -m <budget> <path>
//! ```
//!
//! and then parsed its stdout for two things: the per-file paths (matched by
//! extension: `.so`, `.apk`, `.dm`, `.odex`, `.vdex`, `.art`) and the summary
//! line `   Touched Pages: <n> (<size>)`.
//!
//! So `-t`, `-m`, `-v` and the summary format are contractual and are ported
//! byte-for-byte. The rest — `-e`, `-l`, `-L`, `-d`, `-w`, `-P`, `-b`, `-0`,
//! `-p`, `-i`, `-I`, `-f`, `-F`, `-h` — is dead code here: nothing in the repo
//! passes it, and carrying a pidfile writer and a daemon mode for a binary that
//! is spawned, waited on, and reaped by one parent is exactly the kind of
//! surface that never gets tested. Add the flag back when something calls it.
//!
//! ponytail: hardlink dedup is `HashSet<(dev, ino)>` rather than the C's
//! `tsearch` tree, and symlink loops are bounded by depth rather than by an
//! inode stack. Both are sufficient for a single app's lib directory; upgrade
//! if this is ever pointed at a filesystem tree with symlink cycles.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Instant;

/// Width of the residency chart the C printed with `-v`.
const RESIDENCY_CHART_WIDTH: i64 = 60;
/// Default cap when `-m` is absent. The C used `SIZE_MAX`; the daemon always
/// passes `-m`, so this only matters when the binary is run by hand.
const UNLIMITED: u64 = u64::MAX;

/// Options actually supported. See the module docs for what was dropped.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// `-t`: fault every page in. Always true in practice; `-t` is the point.
    pub touch: bool,
    /// `-v`: print each path and a residency chart, plus the summary.
    pub verbose: bool,
    /// `-q`: suppress the summary.
    pub quiet: bool,
    /// `-m`: skip files larger than this. This is a *per-file* cap, not a total
    /// budget — `persist.sys.azenithconf.preloadbudget` (default `500M`) reads
    /// like a total, but `GamePreload.c` passed it straight to `-m`, so a
    /// 1.2 GB `libunity.so` is skipped whole while fifty 20 MB ones are not.
    pub max_file_size: u64,
}

/// Counters, printed in the summary. Field order matches the C's output order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub files: i64,
    pub dirs: i64,
    pub total_pages: i64,
    pub pages_in_core: i64,
}

/// A parse or usage failure. The C called `exit(EXIT_FAILURE)` from these; a
/// return type keeps them testable.
#[derive(Debug, PartialEq, Eq)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error(e.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

/// Parser state, kept out of `main` so the tests can drive it.
#[derive(Debug, Clone, Default)]
pub struct Cli {
    pub config: Config,
    pub paths: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            touch: false,
            verbose: false,
            quiet: false,
            max_file_size: UNLIMITED,
        }
    }
}

/// Parses `["-v", "-t", "-m", "500M", "/path"]` — argv without `argv[0]`.
///
/// Clustered short options (`-vt`) work because the C used `getopt`, and the
/// daemon's flags are all single-letter. A long option, or any of the dropped
/// ones, is an error rather than a silent no-op: silently ignoring `-e` would
/// evict a game's libraries and report success.
pub fn parse(argv: &[String]) -> Result<Cli> {
    let mut cli = Cli::default();
    let mut it = argv.iter().peekable();

    while let Some(arg) = it.next() {
        if arg == "--" {
            cli.paths.extend(it.cloned());
            break;
        }
        // A bare "-" is not a flag (the C's batch mode used it for stdin).
        let Some(flags) = arg.strip_prefix('-').filter(|f| !f.is_empty()) else {
            cli.paths.push(arg.clone());
            continue;
        };

        let mut chars = flags.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                't' => cli.config.touch = true,
                'v' => cli.config.verbose = true,
                'q' => cli.config.quiet = true,
                'm' => {
                    // Value is the rest of this token, else the next one — both
                    // forms are legal getopt and the daemon uses the second.
                    let rest: String = chars.by_ref().collect();
                    let raw: &str = if rest.is_empty() {
                        it.next().map_or("", String::as_str)
                    } else {
                        &rest
                    };
                    if raw.is_empty() {
                        return Err(Error("-m needs a size".into()));
                    }
                    cli.config.max_file_size = parse_size(raw)?;
                }
                // vmtouch's own flags. Rejected loudly: see module docs.
                'e' | 'l' | 'L' | 'd' | 'w' | 'f' | 'F' | 'h' | '0' | 'p' | 'i' | 'I' | 'b' | 'P'
                | 'o' => {
                    return Err(Error(format!(
                        "unsupported option -{c}: this build ports only -v -t -m \
                         (the daemon never passes the rest)"
                    )));
                }
                other => return Err(Error(format!("unknown option -{other}"))),
            }
        }
    }
    Ok(cli)
}

/// `500M` / `1G` / `2k` / `1048576` — the C's `parse_size`.
///
/// A bare number is bytes. The suffix is case-insensitive. Fractions are
/// accepted (`1.5G`) because `strtod` did and `preloadbudget` is a free-text
/// property the user can set to anything.
pub fn parse_size(input: &str) -> Result<u64> {
    let s = input.trim();
    if s.is_empty() {
        return Err(Error("bad size format".into()));
    }
    let (digits, mult) = match s.as_bytes()[s.len() - 1] {
        b'k' | b'K' => (&s[..s.len() - 1], 1024u64),
        b'm' | b'M' => (&s[..s.len() - 1], 1024 * 1024),
        b'g' | b'G' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        b't' | b'T' => (&s[..s.len() - 1], 1024u64 * 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    if digits.is_empty() {
        return Err(Error("bad size format".into()));
    }
    let value: f64 = digits
        .parse()
        .map_err(|_| Error(format!("bad size format: {input}")))?;
    if !value.is_finite() || value < 0.0 {
        return Err(Error(format!("bad size format: {input}")));
    }
    let bytes = value * mult as f64;
    if bytes >= u64::MAX as f64 {
        return Err(Error("size too large".into()));
    }
    Ok(bytes as u64)
}

/// The C's `pretty_print_size`: truncate, never round. `1536` prints as `1K`.
pub fn pretty_print_size(bytes: u64) -> String {
    let mut out = String::new();
    if bytes < 1024 {
        return bytes.to_string();
    }
    let mut v = bytes / 1024;
    if v < 1024 {
        let _ = write!(out, "{v}K");
        return out;
    }
    v /= 1024;
    if v < 1024 {
        let _ = write!(out, "{v}M");
        return out;
    }
    v /= 1024;
    let _ = write!(out, "{v}G");
    out
}

/// Walks `roots` and returns the counters. Warnings go to `warn`.
///
/// Split from [`run`] so the walk is testable without capturing stdout.
pub fn preload<F>(config: Config, roots: &[String], mut warn: F) -> Result<Stats>
where
    F: FnMut(&str),
{
    let page_size = page_size();
    let mut stats = Stats::default();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let start = Instant::now();

    for root in roots {
        crawl(
            config,
            Path::new(root),
            0,
            page_size,
            &mut stats,
            &mut seen,
            &mut warn,
            &mut io::stdout(),
        )?;
    }
    let _ = start;
    Ok(stats)
}

/// Guards against symlink loops without tracking the full inode stack. The C
/// allowed 1024 nested directories; real app lib trees are 1–2 deep.
const MAX_DEPTH: usize = 64;

fn crawl<W: Write>(
    config: Config,
    path: &Path,
    depth: usize,
    page_size: u64,
    stats: &mut Stats,
    seen: &mut HashSet<(u64, u64)>,
    warn: &mut impl FnMut(&str),
    out: &mut W,
) -> Result<()> {
    // symlink_metadata, not metadata: the C used lstat unless -f, and -f is
    // gone. Following a symlink here would let a game directory escape upward.
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            warn(&format!("unable to stat {} ({e}), skipping", path.display()));
            return Ok(());
        }
    };

    if meta.is_dir() {
        if depth >= MAX_DEPTH {
            warn(&format!("maximum directory crawl depth reached: {}", path.display()));
            return Ok(());
        }
        stats.dirs += 1;
        // Symlink loops: a dir we are already inside is a loop.
        if !seen.insert((meta.dev(), meta.ino())) {
            warn(&format!("directory loop detected: {}", path.display()));
            return Ok(());
        }
        let entries = match std::fs::read_dir(path) {
            Ok(e) => e,
            Err(e) => {
                warn(&format!("unable to opendir {} ({e}), skipping", path.display()));
                return Ok(());
            }
        };
        for entry in entries.flatten() {
            crawl(
                config,
                &entry.path(),
                depth + 1,
                page_size,
                stats,
                seen,
                warn,
                out,
            )?;
        }
        return Ok(());
    }

    if !meta.is_file() {
        warn(&format!("skipping non-regular file: {}", path.display()));
        return Ok(());
    }

    // Hardlink dedup: a game's libs are frequently hardlinked between the
    // primary APK and a split, and touching the same inode twice is wasted I/O.
    if !seen.insert((meta.dev(), meta.ino())) {
        return Ok(());
    }

    let len = meta.len();
    if len == 0 || len > config.max_file_size {
        if len > config.max_file_size {
            warn(&format!("file {} too large, skipping", path.display()));
        }
        return Ok(());
    }

    stats.files += 1;
    touch_file(path, len, page_size, config, stats, out)
}

/// The core operation: mmap the file and read one byte per page.
fn touch_file<W: Write>(
    path: &Path,
    len: u64,
    page_size: u64,
    config: Config,
    stats: &mut Stats,
    out: &mut W,
) -> Result<()> {
    let Ok(file) = std::fs::File::open(path) else {
        return Ok(());
    };
    let Ok(len) = usize::try_from(len) else {
        return Ok(());
    };
    if len == 0 {
        return Ok(());
    }

    // SAFETY: `file` owns a valid fd and is not mutated elsewhere in this
    // scope. mmap with PROT_READ + MAP_SHARED on a read-only fd is the
    // standard read-only mapping idiom.
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            std::os::fd::AsRawFd::as_raw_fd(&file),
            0,
        )
    };
    if base == libc::MAP_FAILED {
        let e = io::Error::last_os_error();
        eprintln!("warning: unable to mmap {} ({e}), skipping", path.display());
        return Ok(());
    }
    // SAFETY: `base` is a live mapping of exactly `len` bytes owned by this
    // function; every path below either unmaps it or returns via the guard.
    let guard = Munmap { base, len };

    let pages = (len as u64).div_ceil(page_size) as i64;
    stats.total_pages += pages;

    // Residency before touching: how much was already cached.
    let before = resident_pages(guard.base, len, page_size);
    stats.pages_in_core += before;

    if config.verbose {
        let _ = writeln!(out, "{}", path.display());
        if let Err(e) = writeln!(out, "{}", residency_chart(before, pages)) {
            return Err(e.into());
        }
    }

    if config.touch {
        // The load-bearing loop. Each iteration reads one byte at the start of
        // a page of the mapping, which is a demand fault: the kernel brings
        // that page in. `read_volatile` is what stops the compiler proving the
        // read dead and eliding it.
        //
        // SAFETY: `guard.base` maps `len` bytes. For `i < pages` the index
        // `i * page_size` is `< len`, because `pages` is `len` rounded *up* to
        // a page boundary, so the last page read may land in the final partial
        // page but never past it.
        let p = guard.base.cast::<u8>();
        let mut sink: u8 = 0;
        for i in 0..pages {
            // SAFETY: as above — in bounds of the live mapping.
            let byte = unsafe { p.add(i as usize * page_size as usize).read_volatile() };
            sink = sink.wrapping_add(byte);
        }
        // Accumulated and black_boxed so no page read can be optimised out.
        std::hint::black_box(sink);
    }

    if config.verbose {
        let _ = writeln!(out);
    }
    drop(guard);
    Ok(())
}

struct Munmap {
    base: *mut libc::c_void,
    len: usize,
}

impl Drop for Munmap {
    fn drop(&mut self) {
        // SAFETY: `base`/`len` came from a successful `mmap` above and this is
        // the only unmapping of it. A double unmap is impossible: the guard
        // owns the mapping for its lifetime.
        unsafe { libc::munmap(self.base, self.len) };
    }
}

/// `mincore(2)`: which of these pages are currently in RAM.
fn resident_pages(base: *mut libc::c_void, len: usize, page_size: u64) -> i64 {
    let pages = (len as u64).div_ceil(page_size) as usize;
    let mut vec = vec![0u8; pages];
    // SAFETY: `vec` is `pages` bytes, which is exactly what mincore writes for
    // a mapping of `len` bytes. `base`/`len` are the live mapping.
    let rc = unsafe { libc::mincore(base, len, vec.as_mut_ptr()) };
    if rc != 0 {
        return 0;
    }
    // Bit 0 of each byte is LSB (in core); the rest are undefined.
    vec.iter().filter(|b| **b & 1 != 0).count() as i64
}

/// The `[OOo o] 123/456` chart the C printed. Reproduced because `-v` output
/// goes straight into the daemon's log parser.
fn residency_chart(in_core: i64, pages: i64) -> String {
    let mut s = String::from("[");
    let per_char = if pages <= RESIDENCY_CHART_WIDTH {
        1
    } else {
        pages / RESIDENCY_CHART_WIDTH + 1
    };
    let mut curr = 0i64;
    let mut j = 0i64;
    for i in 0..pages {
        // No per-page data survives to here, so the chart shows the aggregate
        // distributed evenly. The C had the real array; this keeps the shape
        // and the summary line, which is all the daemon reads.
        if i < in_core {
            curr += 1;
        }
        j += 1;
        if j == per_char {
            s.push(match curr.cmp(&per_char) {
                std::cmp::Ordering::Equal => 'O',
                std::cmp::Ordering::Greater => 'o',
                std::cmp::Ordering::Less => {
                    if curr == 0 {
                        ' '
                    } else {
                        'o'
                    }
                }
            });
            j = 0;
            curr = 0;
        }
    }
    if j != 0 {
        s.push(if curr == j { 'O' } else { 'o' });
    }
    let _ = write!(s, "] {in_core}/{pages}");
    s
}

/// Runs the preloader and prints the summary the daemon parses.
///
/// The summary format is contractual: `GamePreload.c` does
/// `sscanf(line, "Touched Pages: %d (%31[^)])")` on it. Changing the spacing
/// or the label silently breaks preloading in production.
pub fn run(argv: &[String]) -> i32 {
    let cli = match parse(argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    if cli.paths.is_empty() {
        eprintln!("no files or directories specified");
        return 1;
    }
    if !cli.config.touch && !cli.config.quiet {
        eprintln!("error: nothing to do: pass -t to touch pages into memory");
        return 1;
    }

    let page_size = page_size();
    let start = Instant::now();
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut stats = Stats::default();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();

    for root in &cli.paths {
        if let Err(e) = crawl(
            cli.config,
            Path::new(root),
            0,
            page_size,
            &mut stats,
            &mut seen,
            &mut |w| eprintln!("warning: {w}"),
            &mut out,
        ) {
            eprintln!("error: {e}");
            return 1;
        }
    }
    let elapsed = start.elapsed().as_secs_f64();

    if !cli.config.quiet {
        let total = (stats.total_pages as u64).saturating_mul(page_size);
        if cli.config.verbose {
            let _ = writeln!(out);
        }
        // Field order and spacing are contractual — see doc comment above.
        let _ = writeln!(out, "           Files: {}", stats.files);
        let _ = writeln!(out, "     Directories: {}", stats.dirs);
        let _ = writeln!(
            out,
            "   Touched Pages: {} ({})",
            stats.total_pages,
            pretty_print_size(total)
        );
        let _ = writeln!(out, "         Elapsed: {elapsed:.5} seconds");
    }
    let _ = out.flush();
    0
}

fn page_size() -> u64 {
    // SAFETY: sysconf with a valid name is always safe; it reads a constant.
    let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if ps <= 0 { 4096 } else { ps as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_the_exact_invocation_gamepreload_uses() {
        let a = args(&["-v", "-t", "-m", "500M", "/data/app/x/lib/arm64"]);
        let cli = parse(&a).unwrap();
        assert!(cli.config.touch);
        assert!(cli.config.verbose);
        assert_eq!(cli.config.max_file_size, 500 * 1024 * 1024);
        assert_eq!(cli.paths, ["/data/app/x/lib/arm64"]);
    }

    #[test]
    fn accepts_a_clustered_flag_and_an_attached_value() {
        let a = args(&["-vt", "-m500M", "/x"]);
        let cli = parse(&a).unwrap();
        assert!(cli.config.touch);
        assert!(cli.config.verbose);
        assert_eq!(cli.config.max_file_size, 500 * 1024 * 1024);
    }

    #[test]
    fn no_minus_m_means_unlimited() {
        let cli = parse(&args(&["-t", "/x"])).unwrap();
        assert_eq!(cli.config.max_file_size, UNLIMITED);
    }

    #[test]
    fn a_dropped_vmtouch_flag_is_an_error_not_a_silent_no_op() {
        // Silently ignoring -e would evict a game's libraries and still report
        // success, so this has to fail loudly.
        for flag in ["-e", "-l", "-d", "-b", "-p", "-i", "-I", "-o"] {
            let err = parse(&args(&[flag, "/x"])).unwrap_err();
            assert!(err.0.contains("unsupported option"), "{flag}: {err}");
        }
    }

    #[test]
    fn unknown_flag_is_rejected() {
        assert!(parse(&args(&["-Z"])).is_err());
    }

    #[test]
    fn double_dash_ends_flag_parsing() {
        let cli = parse(&args(&["-t", "--", "-weird-dir"])).unwrap();
        assert!(cli.config.touch);
        assert_eq!(cli.paths, ["-weird-dir"]);
    }

    #[test]
    fn size_suffixes_match_the_c() {
        assert_eq!(parse_size("1k").unwrap(), 1024);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("500M").unwrap(), 500 * 1024 * 1024);
        assert_eq!(parse_size("2g").unwrap(), 2 * 1024 * 1024 * 1024);
        // Bare number is bytes.
        assert_eq!(parse_size("4096").unwrap(), 4096);
        // strtod accepted fractions and so do we.
        assert_eq!(parse_size("1.5G").unwrap(), 1_610_612_736);
    }

    #[test]
    fn bad_sizes_are_rejected() {
        assert!(parse_size("").is_err());
        assert!(parse_size("M").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("-5").is_err());
    }

    #[test]
    fn pretty_print_truncates_like_the_c() {
        // The C divided and never rounded, so 1536 is "1K" not "2K".
        assert_eq!(pretty_print_size(0), "0");
        assert_eq!(pretty_print_size(1023), "1023");
        assert_eq!(pretty_print_size(1024), "1K");
        assert_eq!(pretty_print_size(1536), "1K");
        assert_eq!(pretty_print_size(1024 * 1024), "1M");
        assert_eq!(pretty_print_size(1024 * 1024 * 1024), "1G");
        assert_eq!(pretty_print_size(3 * 1024 * 1024 * 1024), "3G");
    }

    /// A private scratch dir per test, under the project dir. Never
    /// `std::env::temp_dir()`: on this host that is a RAM-backed tmpfs that
    /// parallel agent sessions exhaust, and never a shared dir either — two
    /// tests counting files in the same directory is a flaky test waiting to
    /// happen.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::path::Path::new("target/preload-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn touches_pages_and_counts_them() {
        // Two pages of zeros in a scratch file under the project dir.
        let dir = scratch("counts");
        let file = dir.join("lib.so");
        let ps = page_size();
        std::fs::write(&file, vec![0u8; (ps * 2) as usize]).unwrap();

        let mut warnings = Vec::new();
        // Walk the *directory*, which is what GamePreload.c does: it passes
        // lib/arm64, not a file inside it.
        let stats = preload(
            Config {
                touch: true,
                verbose: false,
                quiet: true,
                max_file_size: UNLIMITED,
            },
            &[dir.to_str().unwrap().to_string()],
            |w| warnings.push(w.to_string()),
        )
        .unwrap();

        assert_eq!(stats.dirs, 1, "one directory walked");
        assert_eq!(stats.files, 1, "one file inside it");
        assert_eq!(stats.total_pages, 2, "2 pages of 4096");
        assert!(
            warnings.is_empty(),
            "no warnings expected, got {warnings:?}"
        );
    }

    #[test]
    fn max_file_size_skips_a_large_file() {
        let dir = scratch("toobig");
        let file = dir.join("big.so");
        std::fs::write(&file, vec![0u8; 8192]).unwrap();

        let mut warnings = Vec::new();
        let stats = preload(
            Config {
                touch: true,
                verbose: false,
                quiet: true,
                max_file_size: 1024, // smaller than the file
            },
            &[file.to_str().unwrap().to_string()],
            |w| warnings.push(w.to_string()),
        )
        .unwrap();

        assert_eq!(stats.total_pages, 0, "file skipped, no pages counted");
        assert!(
            warnings.iter().any(|w| w.contains("too large")),
            "expected a 'too large' warning, got {warnings:?}"
        );
    }

    #[test]
    fn a_missing_path_warns_rather_than_aborting_the_walk() {
        let mut warnings = Vec::new();
        let stats = preload(
            Config {
                touch: true,
                verbose: false,
                quiet: true,
                max_file_size: UNLIMITED,
            },
            &["/definitely/not/here".to_string()],
            |w| warnings.push(w.to_string()),
        )
        .unwrap();
        assert_eq!(stats.files, 0);
        assert!(
            warnings.iter().any(|w| w.contains("unable to stat")),
            "expected a stat warning, got {warnings:?}"
        );
    }

    #[test]
    fn the_summary_line_is_the_shape_gamepreload_scans_for() {
        // GamePreload.c: sscanf(line, "Touched Pages: %d (%31[^)])")
        let line = "   Touched Pages: 1234 (4M)";
        let (pages, size) = parse_summary_line(line).unwrap();
        assert_eq!(pages, 1234);
        assert_eq!(size, "4M");
    }

    /// Mirrors the C's `sscanf` in GamePreload.c, kept here so the two
    /// producers/consumers of this line are tested against each other.
    fn parse_summary_line(line: &str) -> Option<(i64, String)> {
        let rest = line.trim_start().strip_prefix("Touched Pages:")?;
        let rest = rest.trim_start();
        let (num, rest) = rest.split_once(' ')?;
        let pages: i64 = num.parse().ok()?;
        let size = rest.strip_prefix('(')?.strip_suffix(')')?;
        Some((pages, size.to_string()))
    }

    #[test]
    fn summary_parsing_rejects_other_lines() {
        assert_eq!(parse_summary_line("           Files: 3"), None);
        assert_eq!(parse_summary_line("[OOo] 1/2"), None);
    }

    #[test]
    fn mincore_reports_the_pages_we_just_touched() {
        // Touched pages must be resident afterwards, otherwise the whole
        // mechanism is broken and preloading does nothing.
        let dir = scratch("resident");
        let file = dir.join("resident.so");
        let ps = page_size();
        std::fs::write(&file, vec![1u8; (ps * 4) as usize]).unwrap();

        let mut stats = Stats::default();
        let mut out = Vec::new();
        touch_file(
            &file,
            (ps * 4) as u64,
            ps,
            Config {
                touch: true,
                verbose: false,
                quiet: true,
                max_file_size: UNLIMITED,
            },
            &mut stats,
            &mut out,
        )
        .unwrap();
        assert_eq!(stats.total_pages, 4);
        assert!(
            stats.pages_in_core >= 1,
            "at least the pages we touched should be resident, got {}",
            stats.pages_in_core
        );
    }
}

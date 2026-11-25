//! Self-test for the Windows WinFsp virtual filesystem.
//!
//! Run with:
//!
//! ```text
//! cargo run --release --bin winfsp_selftest
//! ```
//!
//! It performs two groups of checks:
//!
//! 1. Pure checks of the directory-listing marker logic (no driver needed).
//! 2. A live end-to-end check: it mounts the virtual NAND tree on a free
//!    drive letter through the WinFsp driver and inspects it with ordinary
//!    `std::fs` calls, exactly the way an external tool would.
//!
//! This lives in its own binary target on purpose: the `libtest` harness
//! cannot be linked into this project on Windows (FLTK's `cfl_new.cpp`
//! redefines `operator new`/`operator delete`, which collides with the CRT
//! symbols pulled in by `libtest`).

#[cfg(windows)]
mod imp {
    use nand_flash_viewer::data_provider::DumpDataProvider;
use nand_flash_viewer::fuse_node_windows::{mount_virtual_fs, resume_index};
use nand_flash_viewer::types::FileMetadata;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PAGE_LENGTH: u32 = 512;
const BLOCK_SIZE: u32 = 4;
const TOTAL_PAGES: u64 = 8;
const TOTAL_SIZE: u64 = TOTAL_PAGES * PAGE_LENGTH as u64;

static FAILURES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn check(condition: bool, what: &str) {
    if condition {
        println!("  ok    {what}");
    } else {
        println!("  FAIL  {what}");
        FAILURES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A tiny in-memory provider producing a deterministic byte pattern.
struct PatternProvider {
    data: Vec<u8>,
    meta: FileMetadata,
}

impl DumpDataProvider for PatternProvider {
    fn read_bytes(
        &mut self,
        offset: u64,
        length: u32,
    ) -> nand_flash_viewer::error::Result<Vec<u8>> {
        if offset >= self.data.len() as u64 {
            return Ok(Vec::new());
        }
        let end = (offset + length as u64).min(self.data.len() as u64) as usize;
        Ok(self.data[offset as usize..end].to_vec())
    }

    fn get_metadata(&self) -> FileMetadata {
        self.meta.clone()
    }

    fn cache_identity(&self) -> String {
        "pattern-test".to_string()
    }
}

fn make_provider() -> Arc<Mutex<dyn DumpDataProvider>> {
    let data: Vec<u8> = (0..TOTAL_SIZE).map(|i| (i % 251) as u8).collect();
    let meta = FileMetadata {
        path: "pattern.bin".to_string(),
        size: TOTAL_SIZE,
        page_length: PAGE_LENGTH,
        block_size: BLOCK_SIZE,
        total_pages: TOTAL_PAGES,
        total_blocks: TOTAL_PAGES / BLOCK_SIZE as u64,
        grid_width: 2,
        grid_height: 2,
    };
    Arc::new(Mutex::new(PatternProvider { data, meta }))
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Marker logic: the enumeration must always move forward.
fn check_marker_logic() {
    println!("\n[1/3] directory marker logic");

    let list = names(&[".", "..", "dump.bin", "meta.json"]);
    check(resume_index(&list, None) == 0, "no marker starts at the beginning");
    check(resume_index(&list, Some(".")) == 1, "marker '.' resumes after '.'");
    check(resume_index(&list, Some("..")) == 2, "marker '..' resumes after '..'");
    check(
        resume_index(&list, Some("dump.bin")) == 3,
        "marker 'dump.bin' resumes after it",
    );
    check(
        resume_index(&list, Some("meta.json")) == list.len(),
        "marker at the end ends the listing",
    );
    check(
        resume_index(&list, Some("does-not-exist")) == list.len(),
        "unknown marker ends the listing",
    );
    check(
        resume_index(&list, Some("DUMP.BIN")) == 3,
        "marker matching is case-insensitive",
    );

    // The critical property: replaying the listing with the returned markers
    // must terminate. If the marker were ignored, WinFsp would enumerate
    // forever and hang every process that lists drive letters.
    //
    // `start` is where the current batch began, `end` is one past the last
    // entry handed out. The next batch must start at or after `end`, and the
    // walk must terminate.
    let mut start = 0usize;
    let mut steps = 0usize;
    let mut advanced = true;
    loop {
        let end = (start + 1).min(list.len());
        if end >= list.len() {
            break;
        }
        let marker = list[end - 1].clone();
        let next = resume_index(&list, Some(&marker));
        if next < end {
            advanced = false;
            break;
        }
        start = next;
        steps += 1;
        if steps > 100 {
            break;
        }
    }
    check(advanced, "enumeration never repeats an entry");
    check(steps < 100, "enumeration terminates");
}

/// List a directory, collecting at most `limit` entries.
fn list_bounded(dir: &Path, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let iter = match std::fs::read_dir(dir) {
        Ok(iter) => iter,
        Err(e) => {
            println!("  FAIL  cannot read {}: {e}", dir.display());
            FAILURES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return out;
        }
    };
    for entry in iter {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                println!("  FAIL  directory entry error: {e}");
                FAILURES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                break;
            }
        };
        out.push(entry.file_name().to_string_lossy().to_string());
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Live end-to-end check through a real WinFsp mount.
fn check_live_mount() {
    println!("\n[2/3] live WinFsp mount");

    let letter = match nand_flash_viewer::fuse_node_windows::next_free_drive() {
        Some(l) => l,
        None => {
            println!("  FAIL  no free drive letter");
            FAILURES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return;
        }
    };
    let target = PathBuf::from(format!("{letter}:"));

    let provider = make_provider();
    let (volume, actual) = match mount_virtual_fs(provider, &target) {
        Ok(v) => v,
        Err(e) => {
            println!("  FAIL  mount failed: {e}");
            FAILURES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return;
        }
    };
    println!("  mounted at {}", actual.display());

    // --- root listing ---------------------------------------------------
    let root = list_bounded(&actual, 32);
    println!("  root entries: {root:?}");
    for expected in ["dump.bin", "meta.json", "blocks", "pages"] {
        check(
            root.iter().any(|e| e == expected),
            &format!("root listing contains '{expected}'"),
        );
    }

    // --- meta.json ------------------------------------------------------
    match std::fs::read_to_string(actual.join("meta.json")) {
        Ok(text) => {
            check(text.contains("pattern.bin"), "meta.json describes the dump");
            check(
                text.contains("\"total_pages\": 8") || text.contains("\"total_pages\":8"),
                "meta.json contains the page count",
            );
        }
        Err(e) => check(false, &format!("meta.json is readable: {e}")),
    }

    // --- dump.bin -------------------------------------------------------
    let dump = match std::fs::read(actual.join("dump.bin")) {
        Ok(d) => d,
        Err(e) => {
            check(false, &format!("dump.bin is readable: {e}"));
            Vec::new()
        }
    };
    check(dump.len() == TOTAL_SIZE as usize, "dump.bin has the full size");
    check(
        dump.len() > 8 && dump[0] == 0 && dump[1] == 1 && dump[250] == 250,
        "dump.bin carries the provider bytes",
    );

    // --- blocks ---------------------------------------------------------
    let blocks = list_bounded(&actual.join("blocks"), 16);
    println!("  blocks: {blocks:?}");
    check(blocks.iter().any(|e| e == "block_0000"), "blocks lists block_0000");
    check(blocks.iter().any(|e| e == "block_0001"), "blocks lists block_0001");

    // --- pages ----------------------------------------------------------
    let pages = list_bounded(&actual.join("pages"), 16);
    println!("  pages: {pages:?}");
    check(
        pages.iter().any(|e| e == "page_000000.bin"),
        "pages lists page_000000.bin",
    );

    // --- a page matches the flat dump at the same offset -----------------
    match std::fs::read(actual.join("pages").join("page_000000.bin")) {
        Ok(page) => {
            println!(
                "  page_000000.bin: len={} first16={:?}",
                page.len(),
                &page[..page.len().min(16)]
            );
            println!(
                "  dump.bin[0..16]: {:?}",
                &dump[..dump.len().min(16)]
            );
            check(
                page.len() == PAGE_LENGTH as usize && page == dump[0..PAGE_LENGTH as usize],
                "page_000000.bin matches dump.bin[0..512]",
            );
        }
        Err(e) => check(false, &format!("page_000000.bin is readable: {e}")),
    }

    match std::fs::read(actual.join("pages").join("page_000003.bin")) {
        Ok(page) => {
            let start = 3 * PAGE_LENGTH as usize;
            check(
                page == dump[start..start + PAGE_LENGTH as usize],
                "page_000003.bin matches dump.bin[1536..2048]",
            );
        }
        Err(e) => check(false, &format!("page_000003.bin is readable: {e}")),
    }

    // --- a page inside a block directory ---------------------------------
    match std::fs::read(
        actual.join("blocks").join("block_0001").join("page_00.bin"),
    ) {
        Ok(page) => {
            // block_0001 starts at page 4 -> byte offset 4 * 512
            let start = 4 * PAGE_LENGTH as usize;
            check(
                page == dump[start..start + PAGE_LENGTH as usize],
                "block_0001/page_00.bin matches dump.bin[2048..2560]",
            );
        }
        Err(e) => check(false, &format!("block_0001/page_00.bin is readable: {e}")),
    }

    // --- the volume is read-only -----------------------------------------
    check(
        std::fs::write(actual.join("dump.bin"), b"nope").is_err(),
        "writing to the volume is rejected",
    );

    // --- unmount ---------------------------------------------------------
    drop(volume);
    check(
        !Path::new(&format!("{letter}:\\")).exists(),
        "the mount point disappears after unmount",
    );
}

/// The application's own mount entry point, with a real file-backed provider.
///
/// This is exactly the path `WorkflowEditorState::mount_fuse_node` takes:
/// `ActiveFuseMount::mount(provider, path)` -> the volume is mounted, then
/// `unmount()` removes it again.
fn check_app_mount_entry_point() {
    println!("\n[3/3] application mount entry point (ActiveFuseMount)");

    // Write a small dump file and serve it through the normal
    // FileLoader/FileDataProvider pipeline.
    let path = std::env::temp_dir().join("winfsp_selftest_dump.bin");
    let data: Vec<u8> = (0..(64 * 1024u32)).map(|i| (i % 253) as u8).collect();
    if let Err(e) = std::fs::write(&path, &data) {
        check(false, &format!("can write the test dump: {e}"));
        return;
    }

    let loader = match nand_flash_viewer::FileLoader::new(path.as_path(), 512, 64) {
        Ok(l) => l,
        Err(e) => {
            check(false, &format!("FileLoader opens the dump: {e}"));
            return;
        }
    };
    let provider: Arc<Mutex<dyn DumpDataProvider>> =
        Arc::new(Mutex::new(nand_flash_viewer::FileDataProvider::new(loader)));

    let letter = match nand_flash_viewer::fuse_node_windows::next_free_drive() {
        Some(l) => l,
        None => {
            check(false, "a free drive letter is available");
            return;
        }
    };
    let target = PathBuf::from(format!("{letter}:"));

    let mut mount = match nand_flash_viewer::ActiveFuseMount::mount(provider, &target) {
        Ok(m) => m,
        Err(e) => {
            check(false, &format!("ActiveFuseMount::mount succeeds: {e}"));
            return;
        }
    };
    println!("  mounted at {}", mount.mount_path.display());
    check(mount.is_mounted.load(std::sync::atomic::Ordering::SeqCst), "the mount reports mounted");

    let root = list_bounded(&mount.mount_path, 16);
    check(
        root.iter().any(|e| e == "dump.bin"),
        "the mounted volume lists dump.bin",
    );

    // The served bytes must come from the provider pipeline.
    let served = std::fs::read(mount.mount_path.join("dump.bin")).unwrap_or_default();
    check(served == data, "dump.bin served through the mount equals the file");

    // Unmount through the application API.
    mount.unmount();
    check(
        !mount.is_mounted.load(std::sync::atomic::Ordering::SeqCst),
        "unmount() clears the mounted flag",
    );
    check(
        !Path::new(&format!("{letter}:\\")).exists(),
        "the drive letter is released after unmount()",
    );
}

pub fn main() {
    println!("ZoomViewer WinFsp self-test");

    check_marker_logic();
    check_live_mount();
    check_app_mount_entry_point();

    let failures = FAILURES.load(std::sync::atomic::Ordering::SeqCst);
    println!("\n========================================");
    if failures == 0 {
        println!("ALL CHECKS PASSED");
        std::process::exit(0);
    } else {
        println!("{failures} CHECK(S) FAILED");
        std::process::exit(1);
    }
}
}

#[cfg(windows)]
fn main() {
    imp::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("winfsp_selftest is only supported on Windows.");
}

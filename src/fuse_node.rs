//! Virtual FUSE Filesystem node for ZoomViewer
//!
//! Exposes the active `DumpDataProvider` pipeline (including XOR transforms,
//! block arrangers, pattern search filters, etc.) as a live, virtual directory mount.
//! External tools (Hex editors, binwalk, Perl/Python scripts) can inspect the data
//! directly without requiring full intermediate file exports to disk.

use crate::data_provider::DumpDataProvider;
use crate::types::FileMetadata;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(unix)]
use fuser::{
    spawn_mount2, BackgroundSession, FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData,
    ReplyDirectory, ReplyEntry, ReplyXattr, Request,
};
#[cfg(unix)]
use libc::{ENOENT, ERANGE};
#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────────
// Mathematical Inode Allocation Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const INODE_ROOT: u64 = 1;
pub const INODE_DUMP_BIN: u64 = 2;
pub const INODE_META_JSON: u64 = 3;
pub const INODE_BLOCKS_DIR: u64 = 4;
pub const INODE_PAGES_DIR: u64 = 5;

pub const INODE_BLOCK_DIR_BASE: u64 = 0x0010_0000;
pub const INODE_BLOCK_PAGE_BASE: u64 = 0x1000_0000;
pub const INODE_FLAT_PAGE_BASE: u64 = 0x0100_0000_0000_0000;

/// Categorized identity of an inode in the virtual filesystem
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum InodeKind {
    Root,
    DumpBin,
    MetaJson,
    BlocksDir,
    PagesDir,
    BlockDir(u64),
    BlockPage {
        block_idx: u64,
        page_in_block: u32,
        page_idx: u64,
    },
    FlatPage(u64),
    Unknown,
}

#[inline]
pub fn inode_for_block_dir(block_idx: u64) -> u64 {
    INODE_BLOCK_DIR_BASE + block_idx
}

#[inline]
pub fn inode_for_block_page(block_idx: u64, page_in_block: u32) -> u64 {
    INODE_BLOCK_PAGE_BASE + (block_idx << 16) + (page_in_block as u64)
}

#[inline]
pub fn inode_for_flat_page(page_idx: u64) -> u64 {
    INODE_FLAT_PAGE_BASE + page_idx
}

pub fn parse_inode(
    ino: u64,
    block_size: u32,
    total_pages: u64,
    total_blocks: u64,
) -> InodeKind {
    match ino {
        INODE_ROOT => InodeKind::Root,
        INODE_DUMP_BIN => InodeKind::DumpBin,
        INODE_META_JSON => InodeKind::MetaJson,
        INODE_BLOCKS_DIR => InodeKind::BlocksDir,
        INODE_PAGES_DIR => InodeKind::PagesDir,
        _ if ino >= INODE_FLAT_PAGE_BASE => {
            let page_idx = ino - INODE_FLAT_PAGE_BASE;
            if page_idx < total_pages {
                InodeKind::FlatPage(page_idx)
            } else {
                InodeKind::Unknown
            }
        }
        _ if ino >= INODE_BLOCK_PAGE_BASE => {
            let offset = ino - INODE_BLOCK_PAGE_BASE;
            let block_idx = offset >> 16;
            let page_in_block = (offset & 0xFFFF) as u32;
            let page_idx = block_idx * (block_size as u64) + (page_in_block as u64);
            if block_idx < total_blocks && page_in_block < block_size && page_idx < total_pages {
                InodeKind::BlockPage {
                    block_idx,
                    page_in_block,
                    page_idx,
                }
            } else {
                InodeKind::Unknown
            }
        }
        _ if ino >= INODE_BLOCK_DIR_BASE => {
            let block_idx = ino - INODE_BLOCK_DIR_BASE;
            if block_idx < total_blocks {
                InodeKind::BlockDir(block_idx)
            } else {
                InodeKind::Unknown
            }
        }
        _ => InodeKind::Unknown,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Linux FUSE Implementation (via fuser crate)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(unix)]
pub struct DumpFuseFs {
    provider: Arc<Mutex<dyn DumpDataProvider>>,
    meta: FileMetadata,
    meta_json: Vec<u8>,
    mount_time: SystemTime,
}

#[cfg(unix)]
impl DumpFuseFs {
    pub fn new(provider: Arc<Mutex<dyn DumpDataProvider>>) -> Self {
        let meta = provider.lock().get_metadata();
        let meta_json = serde_json::to_vec_pretty(&meta).unwrap_or_default();
        Self {
            provider,
            meta,
            meta_json,
            mount_time: SystemTime::now(),
        }
    }

    fn file_attr_for_kind(&self, kind: InodeKind, ino: u64) -> Option<FileAttr> {
        let ttl_time = self.mount_time;
        match kind {
            InodeKind::Root => Some(FileAttr {
                ino,
                size: 0,
                blocks: 0,
                atime: ttl_time,
                mtime: ttl_time,
                ctime: ttl_time,
                crtime: ttl_time,
                kind: FileType::Directory,
                perm: 0o555,
                nlink: 4,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: 4096,
                flags: 0,
            }),
            InodeKind::DumpBin => Some(FileAttr {
                ino,
                size: self.meta.size,
                blocks: (self.meta.size + 511) / 512,
                atime: ttl_time,
                mtime: ttl_time,
                ctime: ttl_time,
                crtime: ttl_time,
                kind: FileType::RegularFile,
                perm: 0o444,
                nlink: 1,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: self.meta.page_length.max(4096),
                flags: 0,
            }),
            InodeKind::MetaJson => Some(FileAttr {
                ino,
                size: self.meta_json.len() as u64,
                blocks: (self.meta_json.len() as u64 + 511) / 512,
                atime: ttl_time,
                mtime: ttl_time,
                ctime: ttl_time,
                crtime: ttl_time,
                kind: FileType::RegularFile,
                perm: 0o444,
                nlink: 1,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: 4096,
                flags: 0,
            }),
            InodeKind::BlocksDir => Some(FileAttr {
                ino,
                size: 0,
                blocks: 0,
                atime: ttl_time,
                mtime: ttl_time,
                ctime: ttl_time,
                crtime: ttl_time,
                kind: FileType::Directory,
                perm: 0o555,
                nlink: 2 + self.meta.total_blocks.min(65535) as u32,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: 4096,
                flags: 0,
            }),
            InodeKind::PagesDir => Some(FileAttr {
                ino,
                size: 0,
                blocks: 0,
                atime: ttl_time,
                mtime: ttl_time,
                ctime: ttl_time,
                crtime: ttl_time,
                kind: FileType::Directory,
                perm: 0o555,
                nlink: 2,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: 4096,
                flags: 0,
            }),
            InodeKind::BlockDir(_) => Some(FileAttr {
                ino,
                size: 0,
                blocks: 0,
                atime: ttl_time,
                mtime: ttl_time,
                ctime: ttl_time,
                crtime: ttl_time,
                kind: FileType::Directory,
                perm: 0o555,
                nlink: 2,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: 4096,
                flags: 0,
            }),
            InodeKind::BlockPage { .. } | InodeKind::FlatPage(_) => Some(FileAttr {
                ino,
                size: self.meta.page_length as u64,
                blocks: (self.meta.page_length as u64 + 511) / 512,
                atime: ttl_time,
                mtime: ttl_time,
                ctime: ttl_time,
                crtime: ttl_time,
                kind: FileType::RegularFile,
                perm: 0o444,
                nlink: 1,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: self.meta.page_length.max(512),
                flags: 0,
            }),
            InodeKind::Unknown => None,
        }
    }
}

#[cfg(unix)]
impl Filesystem for DumpFuseFs {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let ttl = Duration::from_secs(1);
        let parent_kind = parse_inode(
            parent,
            self.meta.block_size,
            self.meta.total_pages,
            self.meta.total_blocks,
        );

        match parent_kind {
            InodeKind::Root => match name_str {
                "dump.bin" => {
                    if let Some(attr) = self.file_attr_for_kind(InodeKind::DumpBin, INODE_DUMP_BIN) {
                        reply.entry(&ttl, &attr, 0);
                        return;
                    }
                }
                "meta.json" => {
                    if let Some(attr) = self.file_attr_for_kind(InodeKind::MetaJson, INODE_META_JSON) {
                        reply.entry(&ttl, &attr, 0);
                        return;
                    }
                }
                "blocks" => {
                    if let Some(attr) = self.file_attr_for_kind(InodeKind::BlocksDir, INODE_BLOCKS_DIR) {
                        reply.entry(&ttl, &attr, 0);
                        return;
                    }
                }
                "pages" => {
                    if let Some(attr) = self.file_attr_for_kind(InodeKind::PagesDir, INODE_PAGES_DIR) {
                        reply.entry(&ttl, &attr, 0);
                        return;
                    }
                }
                _ => {}
            },
            InodeKind::BlocksDir => {
                // Parse "block_{}" or "block_{:04}"
                if let Some(rest) = name_str.strip_prefix("block_") {
                    if let Ok(block_idx) = rest.parse::<u64>() {
                        if block_idx < self.meta.total_blocks {
                            let ino = inode_for_block_dir(block_idx);
                            if let Some(attr) = self.file_attr_for_kind(InodeKind::BlockDir(block_idx), ino) {
                                reply.entry(&ttl, &attr, 0);
                                return;
                            }
                        }
                    }
                }
            }
            InodeKind::BlockDir(block_idx) => {
                // Parse "page_{}.bin" or "page_{:02}.bin"
                if let Some(rest) = name_str.strip_prefix("page_").and_then(|s| s.strip_suffix(".bin")) {
                    if let Ok(page_in_block) = rest.parse::<u32>() {
                        if page_in_block < self.meta.block_size {
                            let page_idx = block_idx * (self.meta.block_size as u64) + (page_in_block as u64);
                            if page_idx < self.meta.total_pages {
                                let ino = inode_for_block_page(block_idx, page_in_block);
                                let kind = InodeKind::BlockPage {
                                    block_idx,
                                    page_in_block,
                                    page_idx,
                                };
                                if let Some(attr) = self.file_attr_for_kind(kind, ino) {
                                    reply.entry(&ttl, &attr, 0);
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            InodeKind::PagesDir => {
                // Direct lookup for "page_{}.bin"
                if let Some(rest) = name_str.strip_prefix("page_").and_then(|s| s.strip_suffix(".bin")) {
                    if let Ok(page_idx) = rest.parse::<u64>() {
                        if page_idx < self.meta.total_pages {
                            let ino = inode_for_flat_page(page_idx);
                            if let Some(attr) = self.file_attr_for_kind(InodeKind::FlatPage(page_idx), ino) {
                                reply.entry(&ttl, &attr, 0);
                                return;
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        reply.error(ENOENT);
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        let kind = parse_inode(
            ino,
            self.meta.block_size,
            self.meta.total_pages,
            self.meta.total_blocks,
        );
        if let Some(attr) = self.file_attr_for_kind(kind, ino) {
            reply.attr(&Duration::from_secs(1), &attr);
        } else {
            reply.error(ENOENT);
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        if offset < 0 {
            reply.error(ENOENT);
            return;
        }
        let offset = offset as u64;
        let kind = parse_inode(
            ino,
            self.meta.block_size,
            self.meta.total_pages,
            self.meta.total_blocks,
        );

        match kind {
            InodeKind::DumpBin => {
                if offset >= self.meta.size {
                    reply.data(&[]);
                    return;
                }
                let to_read = (self.meta.size - offset).min(size as u64) as u32;
                match self.provider.lock().read_bytes(offset, to_read) {
                    Ok(bytes) => reply.data(&bytes),
                    Err(e) => {
                        log::error!("FUSE read error on dump.bin: {}", e);
                        reply.error(libc::EIO);
                    }
                }
            }
            InodeKind::MetaJson => {
                let total_len = self.meta_json.len() as u64;
                if offset >= total_len {
                    reply.data(&[]);
                    return;
                }
                let end = (offset + size as u64).min(total_len) as usize;
                reply.data(&self.meta_json[offset as usize..end]);
            }
            InodeKind::BlockPage { page_idx, .. } | InodeKind::FlatPage(page_idx) => {
                let page_len = self.meta.page_length as u64;
                if offset >= page_len {
                    reply.data(&[]);
                    return;
                }
                let to_read = (page_len - offset).min(size as u64) as u32;
                let abs_offset = page_idx * page_len + offset;
                match self.provider.lock().read_bytes(abs_offset, to_read) {
                    Ok(bytes) => reply.data(&bytes),
                    Err(e) => {
                        log::error!("FUSE read error on page #{}: {}", page_idx, e);
                        reply.error(libc::EIO);
                    }
                }
            }
            _ => reply.error(ENOENT),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let kind = parse_inode(
            ino,
            self.meta.block_size,
            self.meta.total_pages,
            self.meta.total_blocks,
        );

        let current_offset = offset;

        match kind {
            InodeKind::Root => {
                let entries = [
                    (INODE_ROOT, FileType::Directory, "."),
                    (INODE_ROOT, FileType::Directory, ".."),
                    (INODE_DUMP_BIN, FileType::RegularFile, "dump.bin"),
                    (INODE_META_JSON, FileType::RegularFile, "meta.json"),
                    (INODE_BLOCKS_DIR, FileType::Directory, "blocks"),
                    (INODE_PAGES_DIR, FileType::Directory, "pages"),
                ];

                for (idx, &(entry_ino, entry_type, entry_name)) in entries.iter().enumerate() {
                    let entry_offset = (idx + 1) as i64;
                    if entry_offset > current_offset {
                        if reply.add(entry_ino, entry_offset, entry_type, entry_name) {
                            break;
                        }
                    }
                }
                reply.ok();
            }
            InodeKind::BlocksDir => {
                if current_offset < 1 {
                    if reply.add(INODE_BLOCKS_DIR, 1, FileType::Directory, ".") {
                        reply.ok();
                        return;
                    }
                }
                if current_offset < 2 {
                    if reply.add(INODE_ROOT, 2, FileType::Directory, "..") {
                        reply.ok();
                        return;
                    }
                }

                let start_block = if current_offset >= 2 {
                    (current_offset - 2) as u64
                } else {
                    0
                };

                for b in start_block..self.meta.total_blocks {
                    let next_offset = 2 + (b as i64) + 1;
                    let name = format!("block_{:04}", b);
                    let b_ino = inode_for_block_dir(b);
                    if reply.add(b_ino, next_offset, FileType::Directory, &name) {
                        break;
                    }
                }
                reply.ok();
            }
            InodeKind::BlockDir(block_idx) => {
                if current_offset < 1 {
                    if reply.add(ino, 1, FileType::Directory, ".") {
                        reply.ok();
                        return;
                    }
                }
                if current_offset < 2 {
                    if reply.add(INODE_BLOCKS_DIR, 2, FileType::Directory, "..") {
                        reply.ok();
                        return;
                    }
                }

                let start_p = if current_offset >= 2 {
                    (current_offset - 2) as u32
                } else {
                    0
                };

                for p in start_p..self.meta.block_size {
                    let next_offset = 2 + (p as i64) + 1;
                    let name = format!("page_{:02}.bin", p);
                    let p_ino = inode_for_block_page(block_idx, p);
                    if reply.add(p_ino, next_offset, FileType::RegularFile, &name) {
                        break;
                    }
                }
                reply.ok();
            }
            InodeKind::PagesDir => {
                if current_offset < 1 {
                    if reply.add(INODE_PAGES_DIR, 1, FileType::Directory, ".") {
                        reply.ok();
                        return;
                    }
                }
                if current_offset < 2 {
                    if reply.add(INODE_ROOT, 2, FileType::Directory, "..") {
                        reply.ok();
                        return;
                    }
                }

                // In hybrid mode, stream directory listings up to min(total_pages, 1000)
                // Any page beyond 1000 is still directly addressable via lookup ("pages/page_XXXXX.bin")
                let max_listed = self.meta.total_pages.min(1000);
                let start_p = if current_offset >= 2 {
                    (current_offset - 2) as u64
                } else {
                    0
                };

                for p in start_p..max_listed {
                    let next_offset = 2 + (p as i64) + 1;
                    let name = format!("page_{:06}.bin", p);
                    let p_ino = inode_for_flat_page(p);
                    if reply.add(p_ino, next_offset, FileType::RegularFile, &name) {
                        break;
                    }
                }
                reply.ok();
            }
            _ => reply.error(ENOENT),
        }
    }

    fn getxattr(&mut self, _req: &Request<'_>, ino: u64, name: &OsStr, size: u32, reply: ReplyXattr) {
        let kind = parse_inode(
            ino,
            self.meta.block_size,
            self.meta.total_pages,
            self.meta.total_blocks,
        );

        if !matches!(kind, InodeKind::Root | InodeKind::DumpBin) {
            reply.error(libc::ENODATA);
            return;
        }

        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(libc::ENODATA);
                return;
            }
        };

        let val = match name_str {
            "user.nand.page_size" => format!("{}", self.meta.page_length),
            "user.nand.block_size" => format!("{}", self.meta.block_size),
            "user.nand.total_pages" => format!("{}", self.meta.total_pages),
            "user.nand.total_size" => format!("{}", self.meta.size),
            _ => {
                reply.error(libc::ENODATA);
                return;
            }
        };

        let bytes = val.as_bytes();
        if size == 0 {
            reply.size(bytes.len() as u32);
        } else if (size as usize) >= bytes.len() {
            reply.data(bytes);
        } else {
            reply.error(ERANGE);
        }
    }

    fn listxattr(&mut self, _req: &Request<'_>, ino: u64, size: u32, reply: ReplyXattr) {
        let kind = parse_inode(
            ino,
            self.meta.block_size,
            self.meta.total_pages,
            self.meta.total_blocks,
        );

        if !matches!(kind, InodeKind::Root | InodeKind::DumpBin) {
            reply.size(0);
            return;
        }

        let xattrs = b"user.nand.page_size\0user.nand.block_size\0user.nand.total_pages\0user.nand.total_size\0";
        if size == 0 {
            reply.size(xattrs.len() as u32);
        } else if (size as usize) >= xattrs.len() {
            reply.data(xattrs);
        } else {
            reply.error(ERANGE);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Active Mount Management
// ─────────────────────────────────────────────────────────────────────────────

/// Manages a running FUSE virtual filesystem mount session
pub struct ActiveFuseMount {
    pub mount_path: PathBuf,
    pub is_mounted: Arc<AtomicBool>,
    pub status: String,
    pub error: Option<String>,
    #[cfg(unix)]
    session: Option<BackgroundSession>,
}

impl std::fmt::Debug for ActiveFuseMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveFuseMount")
            .field("mount_path", &self.mount_path)
            .field("is_mounted", &self.is_mounted.load(Ordering::SeqCst))
            .field("status", &self.status)
            .field("error", &self.error)
            .finish()
    }
}

impl Drop for ActiveFuseMount {
    fn drop(&mut self) {
        self.unmount();
    }
}

impl ActiveFuseMount {
    #[cfg(unix)]
    pub fn mount(
        provider: Arc<Mutex<dyn DumpDataProvider>>,
        mount_path: &Path,
    ) -> Result<Self, String> {
        // Ensure directory exists
        if !mount_path.exists() {
            std::fs::create_dir_all(mount_path)
                .map_err(|e| format!("Failed to create mount directory '{}': {}", mount_path.display(), e))?;
        } else {
            // Attempt cleanup of any previous dead mount on this path
            let _ = std::process::Command::new("fusermount3")
                .arg("-u")
                .arg("-z")
                .arg(mount_path)
                .output();
        }

        let fs = DumpFuseFs::new(provider);
        let options = [
            MountOption::RO,
            MountOption::FSName("zoomviewer".to_string()),
            MountOption::NoExec,
            MountOption::NoAtime,
        ];

        match spawn_mount2(fs, mount_path, &options) {
            Ok(session) => {
                log::info!("Mounted FUSE filesystem at '{}'", mount_path.display());
                Ok(Self {
                    mount_path: mount_path.to_path_buf(),
                    is_mounted: Arc::new(AtomicBool::new(true)),
                    status: "Mounted and ready".to_string(),
                    error: None,
                    session: Some(session),
                })
            }
            Err(e) => {
                let err_msg = format!("Failed to mount FUSE filesystem at '{}': {}", mount_path.display(), e);
                log::error!("{}", err_msg);
                Err(err_msg)
            }
        }
    }

    #[cfg(not(unix))]
    pub fn mount(
        _provider: Arc<Mutex<dyn DumpDataProvider>>,
        mount_path: &Path,
    ) -> Result<Self, String> {
        Err("FUSE virtual filesystem is currently supported on Unix/Linux systems (WinFsp driver required for Windows)".to_string())
    }

    pub fn unmount(&mut self) {
        #[cfg(unix)]
        {
            if let Some(session) = self.session.take() {
                drop(session);
            }
            // Ensure fusermount3 detaches cleanly
            let _ = std::process::Command::new("fusermount3")
                .arg("-u")
                .arg("-z")
                .arg(&self.mount_path)
                .output();
        }

        self.is_mounted.store(false, Ordering::SeqCst);
        self.status = "Unmounted".to_string();
        log::info!("Unmounted FUSE filesystem at '{}'", self.mount_path.display());
    }
}

/// Helper to generate a default mount directory for a node
pub fn default_mount_dir_for_node(node_id: usize) -> PathBuf {
    if let Some(mut base) = dirs_home_cache_mount() {
        base.push(format!("node_{}", node_id));
        base
    } else {
        PathBuf::from(format!(".cache/zoomviewer/mount/node_{}", node_id))
    }
}

fn dirs_home_cache_mount() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        Some(PathBuf::from(home).join(".cache").join("zoomviewer").join("mount"))
    } else {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mathematical_inode_allocation() {
        let total_pages = 262_144;
        let total_blocks = 4096;
        let block_size = 64;

        // Root and fixed inodes
        assert_eq!(parse_inode(INODE_ROOT, block_size, total_pages, total_blocks), InodeKind::Root);
        assert_eq!(parse_inode(INODE_DUMP_BIN, block_size, total_pages, total_blocks), InodeKind::DumpBin);
        assert_eq!(parse_inode(INODE_META_JSON, block_size, total_pages, total_blocks), InodeKind::MetaJson);
        assert_eq!(parse_inode(INODE_BLOCKS_DIR, block_size, total_pages, total_blocks), InodeKind::BlocksDir);
        assert_eq!(parse_inode(INODE_PAGES_DIR, block_size, total_pages, total_blocks), InodeKind::PagesDir);

        // Block directories
        let b0_ino = inode_for_block_dir(0);
        let b42_ino = inode_for_block_dir(42);
        assert_eq!(parse_inode(b0_ino, block_size, total_pages, total_blocks), InodeKind::BlockDir(0));
        assert_eq!(parse_inode(b42_ino, block_size, total_pages, total_blocks), InodeKind::BlockDir(42));

        // Block pages
        let p_ino = inode_for_block_page(10, 5);
        assert_eq!(
            parse_inode(p_ino, block_size, total_pages, total_blocks),
            InodeKind::BlockPage {
                block_idx: 10,
                page_in_block: 5,
                page_idx: 10 * 64 + 5,
            }
        );

        // Flat pages
        let flat_ino = inode_for_flat_page(12345);
        assert_eq!(
            parse_inode(flat_ino, block_size, total_pages, total_blocks),
            InodeKind::FlatPage(12345)
        );

        // Out of bounds checks
        let invalid_block = inode_for_block_dir(999_999);
        assert_eq!(parse_inode(invalid_block, block_size, total_pages, total_blocks), InodeKind::Unknown);

        let invalid_page = inode_for_flat_page(999_999_999);
        assert_eq!(parse_inode(invalid_page, block_size, total_pages, total_blocks), InodeKind::Unknown);
    }

    #[test]
    fn test_default_mount_path_format() {
        let p = default_mount_dir_for_node(7);
        let s = p.to_string_lossy();
        assert!(s.contains("zoomviewer") && s.contains("node_7"));
    }

    struct MockProvider {
        data: Vec<u8>,
        meta: FileMetadata,
    }

    impl DumpDataProvider for MockProvider {
        fn read_bytes(&mut self, offset: u64, length: u32) -> crate::error::Result<Vec<u8>> {
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
            "mock".to_string()
        }
    }

    #[test]
    fn test_mock_provider_virtual_reads() {
        let page_length = 64;
        let block_size = 4;
        let total_pages = 16u64;
        let total_size = total_pages * (page_length as u64);

        let mut data = Vec::with_capacity(total_size as usize);
        for i in 0..total_size {
            data.push((i % 256) as u8);
        }

        let meta = FileMetadata {
            path: "mock.bin".to_string(),
            size: total_size,
            page_length,
            block_size,
            total_pages,
            total_blocks: total_pages / (block_size as u64),
            grid_width: 2,
            grid_height: 2,
        };

        let provider = Arc::new(Mutex::new(MockProvider { data, meta }));

        #[cfg(unix)]
        {
            let fs = DumpFuseFs::new(provider);

            // Test dump.bin attributes
            let dump_attr = fs.file_attr_for_kind(InodeKind::DumpBin, INODE_DUMP_BIN).unwrap();
            assert_eq!(dump_attr.size, total_size);

            // Test page attributes
            let page_attr = fs.file_attr_for_kind(InodeKind::FlatPage(0), INODE_FLAT_PAGE_BASE).unwrap();
            assert_eq!(page_attr.size, page_length as u64);

            // Test meta.json attributes
            let meta_attr = fs.file_attr_for_kind(InodeKind::MetaJson, INODE_META_JSON).unwrap();
            assert!(meta_attr.size > 0);

            // Parse meta.json content
            let parsed_meta: serde_json::Value = serde_json::from_slice(&fs.meta_json).unwrap();
            assert_eq!(parsed_meta["page_length"], page_length);
            assert_eq!(parsed_meta["block_size"], block_size);
        }
    }
}

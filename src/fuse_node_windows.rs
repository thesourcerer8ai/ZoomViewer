//! Windows implementation of the virtual NAND filesystem.
//!
//! Windows has no FUSE kernel module; the equivalent is **WinFsp**, a user-mode
//! filesystem driver that exposes a real drive letter (or a directory) to
//! Windows. This module presents the same virtual tree as the Unix `fuser`
//! backend in [`crate::fuse_node`]:
//!
//! ```text
//! /dump.bin                        whole dump, size = meta.size
//! /meta.json                       metadata of the mounted dump
//! /blocks/block_0000/page_00.bin   page inside block 0
//! /pages/page_000000.bin           flat page view
//! ```
//!
//! The filesystem is read-only: the underlying [`DumpDataProvider`] pipeline
//! (XOR transforms, block arrangers, pattern filters, ...) is exposed live, so
//! external tools (hex editors, `binwalk`, scripts) see exactly what the
//! viewer shows.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use crate::data_provider::DumpDataProvider;
use crate::fuse_node::{
    inode_for_block_dir, inode_for_block_page, inode_for_flat_page, INODE_BLOCKS_DIR,
    INODE_DUMP_BIN, INODE_META_JSON, INODE_PAGES_DIR, INODE_ROOT,
};
use crate::types::FileMetadata;

use winfsp::filesystem::{
    DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, OpenFileInfo, VolumeInfo,
    WideNameInfo,
};
use winfsp::host::{FileSystemHost, FineGuard, VolumeParams};
use winfsp::{FspError, Result as FspResult, U16CStr};
use windows::Win32::Foundation::{HLOCAL, LocalFree, STATUS_OBJECT_NAME_NOT_FOUND};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{GetSecurityDescriptorLength, PSECURITY_DESCRIPTOR};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY,
};
use windows::core::PCWSTR;

/// Maximum number of entries returned for a single directory listing.
///
/// Listing hundreds of thousands of pages would take a long time and produce
/// enormous directory buffers. Direct lookups (`pages\page_000123.bin`) still
/// work for every page, so this only limits what is *listed*.
const MAX_LISTED_ENTRIES: u64 = 1000;

/// Maximum number of entries written into one directory buffer.
const MAX_ENTRIES_PER_BATCH: usize = 256;

/// A node of the virtual tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// Volume root.
    Root,
    /// `\dump.bin` - the whole dump.
    DumpBin,
    /// `\meta.json` - the dump metadata.
    MetaJson,
    /// `\blocks` - per-block directories.
    BlocksDir,
    /// `\pages` - flat page files.
    PagesDir,
    /// `\blocks\block_NNNN` - a single block.
    BlockDir(u64),
    /// A page file inside a block directory.
    BlockPage { page_idx: u64 },
    /// A page file inside the flat `pages` directory.
    FlatPage(u64),
}

impl Node {
    fn is_dir(self) -> bool {
        matches!(
            self,
            Node::Root | Node::BlocksDir | Node::PagesDir | Node::BlockDir(_)
        )
    }
}

/// Handle for an open file or directory.
///
/// Public because it appears in the public `FileSystemContext` impl of
/// [`DumpWinFspFs`].
#[derive(Debug, Clone, Copy)]
pub struct NandFileContext {
    /// Inode number, reported to Windows as the file index.
    pub ino: u64,
    /// Which node of the virtual tree this handle refers to.
    pub node: Node,
}

/// The WinFsp filesystem that serves the virtual NAND tree.
#[derive(Clone)]
pub struct DumpWinFspFs {
    provider: Arc<Mutex<dyn DumpDataProvider>>,
    meta: FileMetadata,
    /// Serialized metadata served as `meta.json`.
    meta_json: Vec<u8>,
    /// Security descriptor handed to Windows for every node.
    security: Vec<u8>,
    /// Timestamp used for all file times.
    created: u64,
}

impl DumpWinFspFs {
    pub fn new(provider: Arc<Mutex<dyn DumpDataProvider>>) -> Self {
        let meta = provider.lock().get_metadata();
        log::info!(
            "WinFsp mount: size={} total_blocks={} total_pages={} page_length={} block_size={}",
            meta.size,
            meta.total_blocks,
            meta.total_pages,
            meta.page_length,
            meta.block_size
        );
        let meta_json = serde_json::to_vec_pretty(&meta).unwrap_or_default();
        Self {
            provider,
            meta,
            meta_json,
            security: build_security_descriptor(),
            created: now_filetime(),
        }
    }

    /// Resolve a WinFsp path (e.g. `\blocks\block_0000\page_00.bin`) to a node.
    fn resolve(&self, path: &U16CStr) -> Option<(u64, Node)> {
        let mut current = (INODE_ROOT, Node::Root);

        for component in path_components(path) {
            let lower = component.to_ascii_lowercase();
            current = match current.1 {
                Node::Root => match lower.as_str() {
                    "dump.bin" => (INODE_DUMP_BIN, Node::DumpBin),
                    "meta.json" => (INODE_META_JSON, Node::MetaJson),
                    "blocks" => (INODE_BLOCKS_DIR, Node::BlocksDir),
                    "pages" => (INODE_PAGES_DIR, Node::PagesDir),
                    _ => return None,
                },
                Node::BlocksDir => {
                    let index = parse_index(&lower, "block_")?;
                    if index >= self.meta.total_blocks {
                        return None;
                    }
                    (inode_for_block_dir(index), Node::BlockDir(index))
                }
                Node::BlockDir(block_idx) => {
                    let index = parse_index(&lower, "page_")?;
                    if index >= self.meta.block_size as u64 {
                        return None;
                    }
                    let page_idx = block_idx * self.meta.block_size as u64 + index;
                    if page_idx >= self.meta.total_pages {
                        return None;
                    }
                    (
                        inode_for_block_page(block_idx, index as u32),
                        Node::BlockPage { page_idx },
                    )
                }
                Node::PagesDir => {
                    let index = parse_index(&lower, "page_")?;
                    if index >= self.meta.total_pages {
                        return None;
                    }
                    (inode_for_flat_page(index), Node::FlatPage(index))
                }
                _ => return None,
            };
        }

        Some(current)
    }

    /// The children of a directory, in listing order.
    fn children(&self, node: Node) -> Vec<(String, Node, u64)> {
        let mut out = Vec::new();
        match node {
            Node::Root => {
                out.push(("dump.bin".to_string(), Node::DumpBin, INODE_DUMP_BIN));
                out.push(("meta.json".to_string(), Node::MetaJson, INODE_META_JSON));
                out.push(("blocks".to_string(), Node::BlocksDir, INODE_BLOCKS_DIR));
                out.push(("pages".to_string(), Node::PagesDir, INODE_PAGES_DIR));
            }
            Node::BlocksDir => {
                let count = self.meta.total_blocks.min(MAX_LISTED_ENTRIES);
                for index in 0..count {
                    out.push((
                        format!("block_{index:04}"),
                        Node::BlockDir(index),
                        inode_for_block_dir(index),
                    ));
                }
            }
            Node::BlockDir(_) => {
                for index in 0..self.meta.block_size as u64 {
                    let page_idx = match node {
                        Node::BlockDir(block_idx) => {
                            block_idx * self.meta.block_size as u64 + index
                        }
                        _ => unreachable!("Node::BlockDir branch"),
                    };
                    if page_idx >= self.meta.total_pages {
                        break;
                    }
                    out.push((
                        format!("page_{index:02}.bin"),
                        Node::BlockPage { page_idx },
                        inode_for_block_page(
                            match node {
                                Node::BlockDir(block_idx) => block_idx,
                                _ => unreachable!("Node::BlockDir branch"),
                            },
                            index as u32,
                        ),
                    ));
                }
            }
            Node::PagesDir => {
                let count = self.meta.total_pages.min(MAX_LISTED_ENTRIES);
                for index in 0..count {
                    out.push((
                        format!("page_{index:06}.bin"),
                        Node::FlatPage(index),
                        inode_for_flat_page(index),
                    ));
                }
            }
            _ => {}
        }
        out
    }

    /// Size of a node in bytes.
    fn size_of(&self, node: Node) -> u64 {
        match node {
            Node::Root | Node::BlocksDir | Node::PagesDir | Node::BlockDir(_) => 0,
            Node::DumpBin => self.meta.size,
            Node::MetaJson => self.meta_json.len() as u64,
            Node::BlockPage { .. } | Node::FlatPage(_) => self.meta.page_length as u64,
        }
    }

    /// Windows file attributes of a node.
    fn attributes_of(&self, node: Node) -> u32 {
        if node.is_dir() {
            FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_READONLY.0
        } else {
            FILE_ATTRIBUTE_NORMAL.0 | FILE_ATTRIBUTE_READONLY.0
        }
    }

    /// Fill a [`FileInfo`] with the attributes of a node.
    fn fill_file_info(&self, node: Node, ino: u64, info: &mut FileInfo) {
        info.file_attributes = self.attributes_of(node);
        info.reparse_tag = 0;
        info.file_size = self.size_of(node);
        info.allocation_size = info.file_size;
        info.creation_time = self.created;
        info.last_access_time = self.created;
        info.last_write_time = self.created;
        info.change_time = self.created;
        info.index_number = ino;
        info.hard_links = 0;
        info.ea_size = 0;
    }
}

impl FileSystemContext for DumpWinFspFs {
    type FileContext = NandFileContext;

    /// Security information for a path.
    ///
    /// Windows uses this for the access check *before* the file is opened, so
    /// the returned `attributes` must describe the node (a directory must be
    /// flagged as `FILE_ATTRIBUTE_DIRECTORY`).
    fn get_security_by_name(
        &self,
        file_name: &U16CStr,
        security_descriptor: Option<&mut [c_void]>,
        _reparse_point_resolver: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
    ) -> FspResult<FileSecurity> {
        let (_, node) = match self.resolve(file_name) {
            Some(resolved) => resolved,
            None => return Err(FspError::NTSTATUS(STATUS_OBJECT_NAME_NOT_FOUND.0)),
        };

        let size = self.security.len() as u64;
        if let Some(buffer) = security_descriptor {
            if (buffer.len() as u64) >= size {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        self.security.as_ptr(),
                        buffer.as_mut_ptr() as *mut u8,
                        self.security.len(),
                    );
                }
            }
        }

        Ok(FileSecurity {
            reparse: false,
            sz_security_descriptor: size,
            attributes: self.attributes_of(node),
        })
    }

    /// Open a file or directory.
    fn open(
        &self,
        file_name: &U16CStr,
        _create_options: u32,
        _granted_access: u32,
        file_info: &mut OpenFileInfo,
    ) -> FspResult<NandFileContext> {
        let (ino, node) = match self.resolve(file_name) {
            Some(resolved) => resolved,
            None => return Err(FspError::NTSTATUS(STATUS_OBJECT_NAME_NOT_FOUND.0)),
        };

        let mut info = FileInfo::default();
        self.fill_file_info(node, ino, &mut info);
        *file_info.as_mut() = info;

        Ok(NandFileContext { ino, node })
    }

    fn close(&self, _context: NandFileContext) {}

    /// Attributes of an open handle.
    fn get_file_info(&self, context: &NandFileContext, file_info: &mut FileInfo) -> FspResult<()> {
        self.fill_file_info(context.node, context.ino, file_info);
        Ok(())
    }

    /// Read file content.
    fn read(
        &self,
        context: &NandFileContext,
        buffer: &mut [u8],
        offset: u64,
    ) -> FspResult<u32> {
        let node = context.node;

        // WinFsp hands out buffers that are aligned to the sector size, which
        // can be larger than the file. Never read past the end of the node.
        let size = self.size_of(node);
        if offset >= size {
            return Ok(0);
        }
        let available = (size - offset) as usize;
        let len = buffer.len().min(available);
        let buffer = &mut buffer[..len];

        match node {
            Node::MetaJson => {
                let start = offset as usize;
                let end = start + len;
                buffer.copy_from_slice(&self.meta_json[start..end]);
                Ok(len as u32)
            }
            Node::DumpBin => self.read_from_provider(offset, buffer),
            Node::BlockPage { page_idx } | Node::FlatPage(page_idx) => {
                let page_len = self.meta.page_length as u64;
                let base = page_idx.saturating_mul(page_len);
                self.read_from_provider(base.saturating_add(offset), buffer)
            }
            _ => Err(FspError::NTSTATUS(STATUS_OBJECT_NAME_NOT_FOUND.0)),
        }
    }

    /// List a directory.
    ///
    /// WinFsp resumes a listing by passing the name of the last returned entry
    /// as the marker; only the entries *after* the marker may be returned.
    /// Ignoring the marker makes the enumeration loop forever.
    fn read_directory(
        &self,
        context: &NandFileContext,
        _pattern: Option<&U16CStr>,
        marker: DirMarker,
        buffer: &mut [u8],
    ) -> FspResult<u32> {
        if !context.node.is_dir() {
            return Err(FspError::NTSTATUS(STATUS_OBJECT_NAME_NOT_FOUND.0));
        }

        // The full listing: ".", ".." and the children of this directory.
        let mut entries: Vec<(String, Option<Node>, u64)> = Vec::new();
        entries.push((".".to_string(), None, context.ino));
        if context.node != Node::Root {
            entries.push(("..".to_string(), None, context.ino));
        }
        for (name, child, ino) in self.children(context.node) {
            entries.push((name, Some(child), ino));
        }

        // Resume after the marker, if any.
        let marker_name = marker.inner_as_cstr().map(|m| String::from_utf16_lossy(m.as_slice()));
        let entry_names: Vec<String> = entries.iter().map(|(name, _, _)| name.clone()).collect();
        let start = resume_index(&entry_names, marker_name.as_deref());

        let mut cursor = 0u32;
        let mut dir_info: DirInfo<255> = DirInfo::new();

        for (name, child, ino) in entries.iter().skip(start) {
            let node = child.unwrap_or(context.node);
            dir_info.reset();
            let mut info = FileInfo::default();
            self.fill_file_info(node, *ino, &mut info);
            *dir_info.file_info_mut() = info;
            dir_info.set_name_raw(name.encode_utf16().collect::<Vec<u16>>().as_slice())?;
            if !dir_info.append_to_buffer(buffer, &mut cursor) {
                // The buffer is full - the caller will ask for the rest.
                break;
            }
            if cursor as usize >= MAX_ENTRIES_PER_BATCH {
                break;
            }
        }

        DirInfo::<255>::finalize_buffer(buffer, &mut cursor);
        Ok(cursor)
    }

    /// Volume information.
    fn get_volume_info(&self, volume_info: &mut VolumeInfo) -> FspResult<()> {
        volume_info.total_size = self.meta.size.max(1);
        // Read-only volume: no free space.
        volume_info.free_size = 0;
        volume_info.set_volume_label("NAND");
        Ok(())
    }

    /// Flush is a no-op for a read-only filesystem.
    fn flush(&self, _context: Option<&NandFileContext>, _file_info: &mut FileInfo) -> FspResult<()> {
        Ok(())
    }
}

impl DumpWinFspFs {
    /// Read through the provider pipeline into `buffer`.
    fn read_from_provider(&self, offset: u64, buffer: &mut [u8]) -> FspResult<u32> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let length = buffer.len().min(u32::MAX as usize) as u32;
        match self.provider.lock().read_bytes(offset, length) {
            Ok(bytes) => {
                let count = bytes.len().min(buffer.len());
                buffer[..count].copy_from_slice(&bytes[..count]);
                Ok(count as u32)
            }
            Err(e) => {
                log::error!("WinFsp read error at offset {offset}: {e}");
                Err(FspError::NTSTATUS(0xC00000E9u32 as i32)) // STATUS_UNEXPECTED_IO_ERROR
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Mounting
// ─────────────────────────────────────────────────────────────────────────────

/// A mounted WinFsp volume. Dropping it unmounts the volume.
pub struct WinFspVolume {
    host: FileSystemHost<DumpWinFspFs, FineGuard>,
}

impl WinFspVolume {
    /// Stop the dispatcher and remove the mount point.
    pub fn unmount(&mut self) {
        self.host.stop();
        self.host.unmount();
    }
}

impl Drop for WinFspVolume {
    fn drop(&mut self) {
        self.unmount();
    }
}

/// Mount the virtual NAND filesystem.
///
/// `requested` may be a drive letter (`Z:`, `Z:\`) or a directory. A
/// directory is mounted using the WinFsp "net" device; if that is not
/// possible the next free drive letter is used instead. The returned path is
/// the location the volume actually ended up at.
pub fn mount_virtual_fs(
    provider: Arc<Mutex<dyn DumpDataProvider>>,
    requested: &Path,
) -> Result<(WinFspVolume, PathBuf), String> {
    winfsp::winfsp_init().map_err(|e| {
        format!(
            "WinFsp is not available ({e}). Install the WinFsp driver: winget install WinFsp.WinFsp"
        )
    })?;

    let fs = DumpWinFspFs::new(provider);

    // 1) An explicit drive letter is used as-is.
    if let Some(letter) = drive_letter_of(requested) {
        let target = format!("{letter}:");
        return match attempt_mount(&fs, None, &target) {
            Some(volume) => Ok((volume, PathBuf::from(format!("{letter}:\\")))),
            None => Err(format!(
                "Failed to mount the NAND filesystem on drive {letter}: the WinFsp driver rejected the mount point"
            )),
        };
    }

    // 2) A directory is mounted with the WinFsp "net" device.
    let directory = requested.to_path_buf();
    if let Err(e) = std::fs::create_dir_all(&directory) {
        return Err(format!(
            "Failed to create the mount directory '{}': {e}",
            directory.display()
        ));
    }
    let directory_str = directory.to_string_lossy().to_string();
    for prefix in [r"\\localhost\ZoomViewer", r"\\ZoomViewer\"] {
        if let Some(volume) = attempt_mount(&fs, Some(prefix), &directory_str) {
            log::info!("Mounted NAND filesystem at '{}'", directory.display());
            return Ok((volume, directory));
        }
    }

    // 3) Fall back to the next free drive letter.
    if let Some(letter) = next_free_drive() {
        let target = format!("{letter}:");
        if let Some(volume) = attempt_mount(&fs, None, &target) {
            log::info!(
                "Mounted NAND filesystem on drive {letter}: (directory mount unavailable)"
            );
            return Ok((volume, PathBuf::from(format!("{letter}:\\"))));
        }
    }

    Err(format!(
        "Failed to mount the NAND filesystem at '{}'. \
         Try a drive letter such as 'Z:' instead.",
        directory.display()
    ))
}

/// Try to create, mount and start a WinFsp volume.
fn attempt_mount(
    fs: &DumpWinFspFs,
    prefix: Option<&str>,
    target: &str,
) -> Option<WinFspVolume> {
    let mut params = VolumeParams::new();
    params
        .filesystem_name("ZoomViewer")
        .case_sensitive_search(false)
        .case_preserved_names(true)
        .unicode_on_disk(true)
        .read_only_volume(true)
        .persistent_acls(true)
        .reparse_points(false)
        .named_streams(false)
        .extended_attributes(false)
        .max_component_length(255)
        .sector_size(4096)
        .sectors_per_allocation_unit(1)
        .volume_creation_time(now_filetime());
    if let Some(prefix) = prefix {
        params.prefix(prefix);
    }

    let mut host = FileSystemHost::<DumpWinFspFs, FineGuard>::new(params, fs.clone()).ok()?;
    host.mount(target).ok()?;
    if host.start().is_err() {
        // The volume is mounted but the dispatcher did not come up - remove the
        // mount point again instead of leaving an orphaned volume behind.
        host.unmount();
        return None;
    }
    Some(WinFspVolume { host })
}

/// Extract a drive letter from a path like `Z:` or `Z:\`.
fn drive_letter_of(path: &Path) -> Option<char> {
    let text = path.to_string_lossy();
    let text = text.trim_end_matches(['\\', '/']);
    let mut chars = text.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    if chars.next()? != ':' {
        return None;
    }
    if chars.next().is_some() {
        return None;
    }
    Some(letter.to_ascii_uppercase())
}

/// Find the first free drive letter, counting down from `Z:`.
///
/// Note: the range must be written as `('D'..='Z')` - `('Z'..='D')` is empty
/// because the start is greater than the end.
pub fn next_free_drive() -> Option<char> {
    ('D'..='Z').rev().find(|letter| {
        !Path::new(&format!("{letter}:\\")).exists()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Split a WinFsp path into its components.
///
/// The volume root is passed as a single backslash (`\`); it has no
/// components.
fn path_components(path: &U16CStr) -> Vec<String> {
    let text = String::from_utf16_lossy(path.as_slice());
    text.split(['\\', '/'])
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| part.to_string())
        .collect()
}

/// Parse `block_0000` / `page_00.bin` style names.
fn parse_index(name: &str, prefix: &str) -> Option<u64> {
    let rest = name.strip_prefix(prefix)?;
    let rest = rest.strip_suffix(".bin").unwrap_or(rest);
    rest.parse::<u64>().ok()
}

/// Index of the first entry that may be returned for a given marker.
///
/// WinFsp resumes a directory listing by passing the name of the last entry
/// it received. Only the entries *after* that name may be returned. An
/// unknown marker means the listing is complete.
///
/// Returning the first entry again (i.e. ignoring the marker) makes the
/// enumeration loop forever, which in turn hangs every process that
/// enumerates drive letters - so this must always move forward.
pub fn resume_index(names: &[String], marker: Option<&str>) -> usize {
    match marker {
        None => 0,
        Some(marker) => match names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(marker))
        {
            Some(index) => index + 1,
            None => names.len(),
        },
    }
}

/// Current time as a Windows FILETIME (100ns units since 1601-01-01).
fn now_filetime() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs() * 10_000_000 + (now.subsec_nanos() as u64) / 100 + 116_444_736_000_000_000
}

/// Build the security descriptor handed to Windows.
///
/// Everyone gets full access; the volume itself is read-only, which the
/// driver enforces separately.
fn build_security_descriptor() -> Vec<u8> {
    const SDDL: &str = "O:BAG:BAD:P(A;;FA;;;WD)";
    let wide: Vec<u16> = SDDL.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let mut size = 0u32;
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            Some(&mut size),
        )
        .expect("failed to build the WinFsp security descriptor");
    }
    let length = unsafe { GetSecurityDescriptorLength(descriptor) } as usize;
    let bytes = unsafe { std::slice::from_raw_parts(descriptor.0 as *const u8, length) }.to_vec();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use winfsp::U16CString;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_marker_starts_at_the_beginning() {
        let list = names(&[".", "..", "dump.bin"]);
        assert_eq!(resume_index(&list, None), 0);
    }

    #[test]
    fn marker_resumes_strictly_after_the_entry() {
        let list = names(&[".", "..", "dump.bin", "meta.json"]);
        assert_eq!(resume_index(&list, Some(".")), 1);
        assert_eq!(resume_index(&list, Some("..")), 2);
        assert_eq!(resume_index(&list, Some("dump.bin")), 3);
    }

    #[test]
    fn marker_at_the_end_ends_the_listing() {
        let list = names(&[".", "dump.bin"]);
        assert_eq!(resume_index(&list, Some("dump.bin")), list.len());
    }

    #[test]
    fn unknown_marker_ends_the_listing() {
        let list = names(&[".", "dump.bin"]);
        assert_eq!(resume_index(&list, Some("does-not-exist")), list.len());
    }

    #[test]
    fn marker_matching_is_case_insensitive() {
        let list = names(&[".", "dump.bin", "meta.json"]);
        assert_eq!(resume_index(&list, Some("DUMP.BIN")), 2);
        assert_eq!(resume_index(&list, Some("META.JSON")), 3);
    }

    /// The enumeration must always move forward, otherwise WinFsp loops
    /// forever and every process that enumerates drive letters hangs.
    #[test]
    fn enumeration_never_repeats_an_entry() {
        let list = names(&[".", "..", "dump.bin", "meta.json", "blocks", "pages"]);
        let mut start = 0usize;
        let mut steps = 0;
        loop {
            let end = (start + 1).min(list.len());
            if end >= list.len() {
                break;
            }
            // The caller received the entry at `end - 1` and passes its name
            // back as the marker; the next batch must start at or after `end`.
            let next = resume_index(&list, Some(&list[end - 1]));
            assert!(next >= end, "listing repeated an entry at step {steps}");
            start = next;
            steps += 1;
            assert!(steps < 100, "listing did not terminate");
        }
    }

    #[test]
    fn path_components_split_on_backslashes() {
        let raw = U16CString::from_str("\\blocks\\block_0000\\page_00.bin").unwrap();
        assert_eq!(
            path_components(raw.as_ucstr()),
            names(&["blocks", "block_0000", "page_00.bin"])
        );
    }

    #[test]
    fn path_components_of_root_are_empty() {
        let raw = U16CString::from_str("\\").unwrap();
        assert!(path_components(raw.as_ucstr()).is_empty());
    }

    #[test]
    fn indices_are_parsed_from_names() {
        assert_eq!(parse_index("block_0007", "block_"), Some(7));
        assert_eq!(parse_index("page_42.bin", "page_"), Some(42));
        assert_eq!(parse_index("meta.json", "page_"), None);
    }
}

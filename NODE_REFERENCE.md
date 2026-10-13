# ZoomViewer Workflow Node Reference

This document describes every `WorkflowNodeKind` variant: what it does, what
provider it creates, how it transforms addresses, and how `trace_byte` maps
an output byte offset back to its source.

---

## Input

**What it does**: Reads a raw NAND dump file from disk and feeds it into the
pipeline. This is always a leaf node — it has no upstream inputs.

**Inputs**: None (reads directly from `file_path`).

**Output provider**: `FileDataProvider` streaming raw bytes from a single file.

**Configuration**:
- `file_path` — path to the binary dump file.
- `page_length` — bytes per NAND page (e.g. 512, 2048).
- `block_size` — pages per NAND block (e.g. 64, 128).

**Coordinate-space behaviour for `trace_byte`**:
Direct mapping — `absolute_offset` in the virtual stream equals the file offset.

```
block          = (offset / page_length) / block_size
page_in_block  = (offset / page_length) % block_size
offset_in_page = offset % page_length
```

Returns a single `BacktrackAddress` with `is_leaf = true`.

---

## XOR Transform

**What it does**: XORs one primary input with up to two secondary inputs
and/or a static byte pattern. Used to apply or reverse ECC / scrambling.

**Inputs**: One primary (required), up to two secondaries (optional).

**Output provider**: `XorDataProvider` — output size equals the primary size.

**Configuration**:
- `pattern_hex` — optional static XOR pattern (hex string, e.g. `"FF00"`).

**Coordinate-space behaviour for `trace_byte`**:
The clicked output offset maps to the primary at the same offset, plus each
secondary using the offset rules below.

### XOR Secondary Offset Rules

Let `off` = output offset, `page_len` = primary page length,
`block_bytes` = `page_len × block_size`, `sec_size` = secondary total size.

- **Rule 1** (`sec_size ≤ page_len`): `sec_offset = off % page_len`;
  active only if `(off % page_len) < sec_size`.
- **Rule 2** (`sec_size ≤ block_bytes`): `sec_offset = off % block_bytes`;
  active only if `(off % block_bytes) < sec_size`.
- **Rule 3** (`sec_size > block_bytes`): `sec_offset = off`;
  active only if `off < sec_size`.

If a secondary is not active for the given offset it contributes no address.

---

## Output Viewer

**What it does**: Sink / tap node. Marks the end of a processing pipeline and
exposes it to the NAND Viewer, Hex Tab, and other analysis tools. Multiple
Output Viewers can exist; the "active" one (selected in the Workflow Editor)
is what is displayed.

**Inputs**: One upstream node.

**Output provider**: Passes through transparently — no byte transformation.

**Configuration**:
- `name` — label shown in the tab strip and status bar.

**Coordinate-space behaviour for `trace_byte`**: Transparent. Delegates
unchanged to the upstream provider.

---

## Pattern Search (Search Node)

**What it does**: Searches the upstream byte stream for a pattern (ASCII or
hex). Depending on `output_mode`, the output is either every page that
contains a match, or one context page per match.

**Inputs**: One upstream node.

**Output provider**:
- `SearchFilteredDataProvider` — one page per matching source page
  (`AllMatchingPages` mode).
- `SearchContextDataProvider` — one page per match with configurable leading
  and trailing context bytes (`ContextPages` mode).

**Configuration**:
- `search_pattern` — pattern string.
- `is_hex` — interpret pattern as hex bytes.
- `case_sensitive` — case-sensitive ASCII match.
- `max_matches` — upper limit on stored results.
- `output_mode` — `AllMatchingPages` or `ContextPages`.
- `bytes_before` / `bytes_after` — context window for `ContextPages` mode.

**Coordinate-space behaviour for `trace_byte`**:
- `SearchFilteredDataProvider`: output page index → source page via
  `matching_pages[out_page]`, then delegates.
- `SearchContextDataProvider`: per-match window arithmetic maps the in-page
  position back to the source offset, then delegates.

---

## Block Arranger

**What it does**: Changes how blocks are laid out visually in the NAND Viewer
(adjusts `grid_width × grid_height` and stripe width). Does **not** alter the
byte stream at all.

**Inputs**: One upstream node.

**Output provider**: Returns the upstream provider unchanged — same bytes,
same offsets.

**Configuration**:
- `grid_width`, `grid_height` — grid dimensions for the viewer.
- `stripe_width` — visual stripe grouping.

**Coordinate-space behaviour for `trace_byte`**: Transparent. The provider
returned *is* the upstream provider, so `trace_byte` runs directly on it.

---

## File Export

**What it does**: Writes the upstream byte stream to a binary output file.
Sink node — used to export filtered or XOR-processed dumps to disk.

**Inputs**: One upstream node.

**Output provider**: Passes through transparently.

**Configuration**:
- `export_path` — destination file path.
- `auto_export` — trigger export automatically when the pipeline changes.

**Coordinate-space behaviour for `trace_byte`**: Transparent — delegates
unchanged to upstream.

---

## FUSE Mount

**What it does**: Mounts the upstream byte stream as a FUSE filesystem so
external tools (hex editors, `dd`, etc.) can read the processed data directly.

**Inputs**: One upstream node.

**Output provider**: Passes through transparently.

**Configuration**:
- `mount_path` — directory to mount at.
- `auto_mount` — trigger mount automatically.

**Coordinate-space behaviour for `trace_byte`**: Transparent — delegates
unchanged to upstream.

---

## Concatenate

**What it does**: Concatenates two or more input streams end-to-end. Each
input is zero-padded to the length of the **longest** input so all slots have
equal size before concatenation.

**Inputs**: Two or more upstream nodes.

**Output provider**: `ConcatenateDataProvider` — total size is
`num_inputs × max_input_pages × page_length`.

**Configuration**: None (topology only — connect two or more inputs).

**Coordinate-space behaviour for `trace_byte`**:

```
padded_input_size = max_input_pages × page_length
input_slot        = offset / padded_input_size
local_offset      = offset % padded_input_size
```

Delegates to `inputs[input_slot]` at `local_offset`. If `local_offset ≥` the
actual input size for that slot, the byte is in the zero-padding region and
returns an empty address list.

---

## Modulo Page Filter

**What it does**: Keeps only every *N*-th page starting from page *M*. Useful
for extracting even / odd pages or any regular stride.

**Inputs**: One upstream node.

**Output provider**: `ModuloPageFilterDataProvider` — a contiguous subset of
source pages re-indexed from 0.

**Configuration**:
- `modulo` — step *N* (keep one page every *N* pages).
- `offset` — first page index *M* to include.

**Coordinate-space behaviour for `trace_byte`**:

```
out_page      = offset / page_length
src_page      = selected_pages[out_page]   (pre-computed at build time)
source_offset = src_page × page_length + (offset % page_length)
```

Delegates to upstream at `source_offset`.

---

## NAND Reader

**What it does**: Reads raw NAND data directly from a hardware device over a
serial port, writes it to a dump file, and then exposes that file as a
`FileDataProvider` in the pipeline.

**Inputs**: None (hardware source — this is a leaf node).

**Output provider**: `FileDataProvider` on the written dump file.

**Configuration**:
- `device_path` — serial / USB device path.
- `dump_filename` — path where the dump is written.
- `page_size` — bytes per page as reported by the hardware.
- `pages_per_block` — pages per block as reported by the hardware.

**Coordinate-space behaviour for `trace_byte`**: Same as **Input** — direct
file-offset mapping, `is_leaf = true`.

---

## Pattern Healing

**What it does**: Scans the upstream stream for repeating bad-block patterns
(e.g. OEM fill bytes in DATA segments) and replaces them with zeros. Makes
real content easier to see in the NAND Viewer and Hex Tab. Uses page structure
records from the Page Structure Editor to identify DATA segments.

**Inputs**: One upstream node.

**Output provider**: `PatternHealingDataProvider` — byte-identical to
upstream except healed DATA-segment bytes are replaced with `0x00`.

**Configuration**: None directly — reads segment layout from the shared
`PageStructureTabState`.

**Coordinate-space behaviour for `trace_byte`**: Transparent — delegates
unchanged to upstream (byte offsets are not remapped; only values change).

---

## Block Selector

**What it does**: Forwards only a user-specified subset of source blocks,
emitting them contiguously starting at output block 0.

**Inputs**: One upstream node.

**Output provider**: `BlockSelectorDataProvider` — selected blocks only,
re-indexed 0-based.

**Configuration**:
- `blocks` — comma / range spec string, e.g. `"0,2,5-10"`.

**Coordinate-space behaviour for `trace_byte`**:

```
out_block     = offset / block_bytes
src_block     = selected_blocks[out_block]   (pre-computed at build time)
source_offset = src_block × block_bytes + (offset % block_bytes)
```

Delegates to upstream at `source_offset`.

---

## Block Permutation

**What it does**: Emits source blocks in a triangular enumeration pattern to
generate all non-empty subsets. Block *i* appears *N − i* times (ordered
mode) or by level (unordered mode). Useful for brute-force XOR key search
across many block combinations.

**Inputs**: One upstream node.

**Output provider**: `BlockPermutationDataProvider` — *N×(N+1)/2* total
output blocks.

**Configuration**:
- `ordered` — `true` = group by block index, `false` = group by level.

**Coordinate-space behaviour for `trace_byte`**:

```
out_block     = offset / block_bytes
src_block     = source_block_for(out_block)   (triangular index mapping)
source_offset = src_block × block_bytes + (offset % block_bytes)
```

Delegates to upstream at `source_offset`.

---

## Sector Pattern Search

**What it does**: Searches for sector-level patterns (OEM headers, magic
bytes, etc.) using a custom sector-aware matching algorithm. Filters the
output to only pages that match.

**Inputs**: One upstream node.

**Output provider**: `SearchFilteredDataProvider` wrapping the upstream —
only matching pages are kept.

**Configuration**:
- `log_path` — file path for match log output.
- `max_matches` — upper limit on stored results.
- `results` — stored match list.

**Coordinate-space behaviour for `trace_byte`**: Delegates via
`SearchFilteredDataProvider::trace_byte` — same page-index translation as
Pattern Search in `AllMatchingPages` mode.

---

## Entropy Analysis

**What it does**: Computes per-page Shannon entropy on the fly. In
pass-through mode (`cluster = All`) it is a transparent tap that adds entropy
metadata for the Hex Tab sidebar. In filter mode (any specific cluster) it
outputs only pages whose entropy falls in the selected cluster, behaving like
a filter node.

**Inputs**: One upstream node.

**Output provider**:
- **All mode** — `EntropyDataProvider` (transparent pass-through).
- **Filter mode** — `SearchFilteredDataProvider` wrapping
  `EntropyDataProvider` (only pages in the chosen entropy cluster).

**Configuration**:
- `log_path` — entropy CSV output file path.
- `compute_32bit` — use 32-bit entropy variant.
- `cluster_filter` — entropy cluster to keep (`All` = no filtering;
  `VeryLow`, `Low`, `Medium`, `High`, `VeryHigh` = filter).

**Coordinate-space behaviour for `trace_byte`**:

- **All mode**: `EntropyDataProvider::trace_byte` is transparent — delegates
  to the upstream provider at the same offset.
- **Filter mode**: `SearchFilteredDataProvider::trace_byte` translates the
  filtered output page index back to the source page, then
  `EntropyDataProvider::trace_byte` passes through to the upstream Input.

The full trace chain `SearchFilteredDataProvider → EntropyDataProvider →
upstream` is traversed automatically because every layer implements
`trace_byte`.

> **Note**: In filter mode the provider chain is
> `SearchFilteredDataProvider → EntropyDataProvider → upstream`.
> Both layers implement `trace_byte`, so the full provenance chain is
> recovered without any special-casing in the timer loop.

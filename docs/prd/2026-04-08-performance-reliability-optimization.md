# Performance & Reliability Optimization

Created: 2026-04-08
Category: Infrastructure
Status: Final
Research: Deep

## Problem Statement

fatx-rs is an in-development Rust toolkit for reading, writing, and mounting FATX/XTAF filesystems on Xbox/Xbox 360 drives connected via USB to macOS. The codebase is under active development — core operations work but may be buggy, incomplete, or unreliable in some areas. The current implementation has known reliability and performance gaps: NFS mount stalls and stale mount deadlocks that can freeze Finder (requiring a reboot), sluggish directory browsing and file transfers over the NFS mount, global mutex contention serializing all NFS operations, suboptimal I/O sizing for USB devices, and unnecessary dependency weight. The FATX/XTAF filesystem format itself is well-understood and well-documented — the challenge is building a reliable, performant implementation on macOS, not reverse-engineering the format.

## Core User Flows

### Flow 1: Mount and Browse Xbox 360 Drive
1. User connects Xbox 360 USB drive to Mac
2. User runs `fatx mount /dev/rdiskN`
3. Drive appears in Finder as a mounted volume
4. User browses directories — folder contents load quickly (<1s for any directory)
5. User copies files to/from the drive at near-USB bandwidth
6. User ejects the volume from Finder or Ctrl-C's the process
7. Volume unmounts cleanly — no stale mount, no Finder hang

### Flow 2: Recover from Crash
1. fatx-mount crashes or is killed (OOM, signal, power loss)
2. The mount is automatically cleaned up — no zombie NFS mount
3. User re-runs `fatx mount` without needing to reboot
4. Previous session's writes were either fully flushed or cleanly lost (no partial/corrupt writes)

### Flow 3: Large File Transfer
1. User copies a 4GB game file from Mac to Xbox 360 drive via Finder drag-and-drop
2. Transfer proceeds at sustained USB throughput (not degrading over time)
3. NFS/FUSE does not return ESTALE or timeout errors during transfer
4. FAT is not flushed to disk on every 128KB write chunk
5. Progress bar in Finder reflects actual progress

### Flow 4: Concurrent Multi-Game Bulk Transfer
1. User selects multiple Game on Demand directories (each containing subdirectories and large ISO-derived files, typically 4-8GB per game) in Finder
2. User drags all of them onto the mounted Xbox 360 volume simultaneously
3. Finder initiates concurrent copy operations — multiple directory trees writing in parallel
4. Directory creation for nested folder structures does not deadlock or serialize excessively against ongoing file writes
5. FAT allocation under concurrent pressure does not fragment excessively — `prev_free` hint advances linearly, keeping related clusters near each other
6. Dirty write buffer handles multiple files accumulating concurrently without unbounded memory growth (bounded cache evicts, flush task drains)
7. Periodic FAT flush writes only dirty ranges, not the entire FAT — even with many files being allocated simultaneously
8. No ESTALE, EIO, or timeout errors across any of the concurrent transfers
9. If one file's write fails (e.g., disk full), other in-flight transfers complete or fail gracefully — no corruption of already-written files
10. Total throughput approaches USB bandwidth limit, not 1/Nth of it due to lock contention

## Scope

### In Scope

**I/O Layer (fatxlib)**
- Cluster-sized read buffers (16KB+) instead of per-entry 512B reads
- macOS-native I/O configuration via `nix` crate: `F_NOCACHE` to bypass useless kernel cache, `F_RDAHEAD` control, `DKIOCGETPHYSICALBLOCKSIZE` / `DKIOCGETMAXBYTECOUNTREAD` ioctls for device-optimal I/O sizing
- 4KB page-aligned I/O (required by `F_NOCACHE` on macOS)
- Query `DKIOCGETBLOCKCOUNT` for reliable device size detection (replacing `seek(End(0))` workaround)

**FAT Allocation & Flush (fatxlib)**
- `prev_free` hint: store last allocated cluster, start next scan from there (next-fit, matching Linux FAT driver)
- Free-cluster bitmap: secondary `BitVec` (1 bit/cluster, ~3.6MB for 500GB partition) for 32x faster free cluster scanning
- Cached free cluster count: maintain incrementally on alloc/free instead of full FAT scan in `stats()`
- Dirty-range tracking: track which byte ranges of the FAT cache changed, flush only dirty 4KB pages instead of the entire 120-231MB FAT

**Concurrency Model (fatx-mount)**
- Replace `Arc<Mutex<FatxVolume>>` with `Arc<parking_lot::RwLock<FatxVolume>>` — concurrent NFS reads, exclusive writes, task-fair scheduling (prevents writer starvation during read bursts)
- Replace `Arc<Mutex<HashMap>>` file_cache and dir_cache with `quick_cache::sync::Cache` — bounded by weighted size, internally sharded (no external lock), LRU eviction
- Replace `Arc<Mutex<DirtyFileMap>>` with `Arc<parking_lot::Mutex<DirtyFileMap>>` — no poisoning, faster uncontended acquire
- Adopt `bytes::Bytes` for file cache values — zero-copy NFS read responses via refcounted slicing instead of `Vec<u8>` cloning

**Mount Lifecycle (fatx-mount)**
- Replace nfsserve NFS mount with FUSE-T integration using embedded framework (`/Library/Frameworks/fuse-t.framework`)
- FUSE-T auto-unmounts on process death — structurally eliminates the stale NFS mount deadlock that can freeze Finder and require a reboot
- Implement FUSE operations (getattr, readdir, read, write, mkdir, unlink, rename, statfs) backed by fatxlib
- Rust FFI bindings to libfuse-t C API (or use `fuser` crate with FUSE-T compatibility layer)

**Dependency Cleanup**
- Replace `chrono` with `time` crate (only 2 cold-path call sites for FAT timestamp encoding)
- Add `nix` (macOS ioctls/fcntl), `parking_lot` (fast sync primitives), `quick_cache` (bounded concurrent caches), `bytes` (zero-copy buffers)
- Remove `nfsserve` dependency after FUSE-T migration

### Explicitly Out of Scope
- **Streamed file I/O** — keeping full-file `Vec<u8>` reads for now; streamed cluster-chain reads are a separate optimization pass
- **Trait-based I/O backend** — raw device vs file image divergence is deferred; both use the same code path with different alignment
- **TUI browser** — existing ratatui TUI is in development, polish is deferred
- **Windows/Linux support** — macOS-only target, using platform-specific APIs
- **Contiguity detection** — extent-based single-write optimization deferred
- **Apple File Provider / FSKit** — future macOS 26+ path via FUSE-T's FSKit backend
- **NFS server fallback mode** — FUSE-T is the sole mount mechanism after migration

## Technical Context

### Relevant Architecture
- **Cargo workspace** with 4 crates: `fatxlib` (core), `fatx-cli` (CLI), `fatx-mount` (mount server), `fatx-mkimage` (image generator)
- `fatxlib/src/volume.rs` (~1400 lines) contains all I/O, FAT operations, and directory entry handling
- `fatx-mount/src/main.rs` (~1900 lines) contains NFS server, caching, write buffering, mount lifecycle, signal handling
- FAT cache is a `Vec<u8>` loaded entirely at `open()` time — correct for USB latency, but flush is all-or-nothing
- All NFS operations go through `Arc<Mutex<FatxVolume<File>>>` — single global lock
- **Development state:** The codebase is under active development. Core read/write/mount operations work for common cases but may have bugs or incomplete edge case handling. This optimization pass should expect to encounter and fix issues in existing code, not just layer optimizations on top of a stable base.

### Constraints
- macOS raw devices (`/dev/rdiskN`) require 512-byte sector-aligned I/O; `F_NOCACHE` raises this to 4096-byte page alignment
- USB I/O is inherently serial at the device level — concurrent reads at the Rust level only help for cache-hit paths
- FUSE-T embedded framework must be bundled into the build — no user-facing dependency
- `mmap` does NOT work on macOS raw block devices (returns `ENODEV`) — must use `read`/`write` syscalls
- `seek(SeekFrom::End(0))` returns 0 for raw block devices — must use `DKIOCGETBLOCKCOUNT` ioctl
- FATX has no FSInfo sector — `prev_free` and free count must be maintained in memory only

### Existing Code
- Sector-aligned I/O: `volume.rs` `read_at`/`write_at` methods (already handle 512B alignment, needs widening to 4KB)
- FAT cache: `volume.rs` `fat_cache: Vec<u8>`, `read_fat_entry`/`write_fat_entry` methods
- Allocation: `volume.rs` `allocate_cluster`/`allocate_chain` (linear scan from cluster 0)
- NFS caching: `main.rs` `file_cache`, `dir_cache`, `dirty_files` HashMaps
- Mount lifecycle: `main.rs` signal handler on dedicated thread, panic handler, watchdog task
- Endian helpers: `volume.rs` `read_u16`/`read_u32`/`write_u16_bytes`/`write_u32_bytes`

### Prior Art
- **Linux FAT driver** (`fs/fat/fatent.c`): next-fit allocation via `prev_free`, block-based caching, batch dirty flush, incremental free count — proven patterns to adopt
- **No existing FATX tool** uses `prev_free`, free-cluster bitmaps, dirty-range tracking, or cached free counts — all do linear scans. fatx-rs would be the first.
- **FUSE-T**: kext-less FUSE for macOS, auto-unmounts on process death, supports embedded framework distribution. Used by multiple macOS filesystem projects.
- **nfsserve** (current): only Rust NFS server crate, maintained by HuggingFace, functional but no mount lifecycle management

## Key Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Keep full FAT cache vs block-based | Full cache | USB latency (1-10ms/transfer) makes on-demand block reads impractical. 120MB for a 500GB drive is acceptable on modern Macs. Optimize the cache (dirty tracking, bitmap), don't replace it. |
| nfsserve vs FUSE-T | FUSE-T with embedded framework | FUSE-T auto-unmounts on process death, structurally eliminating the catastrophic stale mount deadlock. Embedded framework means no user-facing dependency. |
| Mutex vs RwLock for volume | parking_lot::RwLock | Concurrent NFS reads don't need exclusive access. parking_lot is strictly better on macOS (no allocation, task-fair, no poisoning). Already a transitive dependency. |
| Custom cache vs crate | quick_cache | Bounded by weighted size (critical for variable-size file cache entries), internally sharded, no external lock needed. Smaller dep tree than moka. |
| chrono vs time vs manual | time crate | Only 2 call sites, cold path. time is lighter than chrono, safer than manual bit manipulation. Low priority but clean. |
| FUSE-T binding approach | Evaluate fuser + FUSE-T compat first | fuser provides a Rust FUSE trait; FUSE-T provides libfuse compatibility. If fuser works with FUSE-T on macOS, this is the lowest-effort path. Fallback: direct FFI to libfuse-t C API. |
| F_NOCACHE | Enable on raw devices | Kernel cache is useless for FATX data. Our app-level caches are more effective. Frees kernel memory. Requires widening alignment from 512B to 4KB. |
| Async I/O (io_uring / kqueue) | Stay with spawn_blocking | macOS has no io_uring. kqueue doesn't work for raw block devices. POSIX AIO uses kernel threads internally — no benefit over tokio's thread pool. Current approach is correct. |
| Streamed file I/O | Deferred | Would reduce peak memory for large files but requires restructuring `read_file` return type. Not needed for this pass — the caching layer handles NFS chunking. |

## Research Findings

### macOS Block Device I/O
- `F_NOCACHE` bypasses kernel UBC — recommended since the kernel can't interpret FATX. Raises alignment requirement to 4096 bytes.
- `F_RDAHEAD` should be disabled for random FAT access, enabled for sequential reads (FAT loading, large file reads).
- `DKIOCGETIOMINSATURATIONBYTECOUNT` returns the kernel's per-device estimate of optimal I/O size — query at open time.
- `mmap` returns `ENODEV` on raw block devices. Only viable for file-backed test images.
- `nix` crate (0.31.2) provides clean wrappers for all needed macOS APIs: `FcntlArg::F_NOCACHE`, `ioctl_read!` macro, `pread`/`pwrite`.
- Current 512B reads over USB are 32-256x suboptimal due to per-transfer overhead. 128KB-1MB reads approach max USB throughput.

### NFS Server & Mount Alternatives
- nfsserve is the only Rust NFS server crate. No alternatives exist.
- FUSE-T is the most promising mount mechanism: kext-less, auto-unmounts on process death, supports embedded framework distribution.
- fuser (Rust FUSE library) lists macOS as "untested" and requires macFUSE kext — but may work with FUSE-T's libfuse compatibility layer.
- fuse3 is Linux-only. Apple File Provider is designed for cloud sync, not block devices.
- rclone and sshfs handle stale mounts via signal handlers + FUSE auto-unmount. The pattern: tie mount lifecycle to process lifecycle.

### FATX Implementation Survey
- 9 open-source FATX implementations catalogued. All load full FAT into memory. None use allocation hints, bitmaps, or dirty-range tracking.
- Linux FAT driver uses next-fit allocation via `prev_free`, block-based caching, batch dirty flush, incremental free count.
- FAT32 FSInfo provides on-disk `Nxt_Free` and `Free_Count` hints. FATX has no equivalent — must be in-memory only.
- exFAT's allocation bitmap (1 bit/cluster) is 32x more compact to scan than FAT32 entries. fatx-rs can adopt this pattern in-memory.

### Rust Caching & Concurrency Patterns
- `quick_cache` is the best fit: lightweight, async-compatible, bounded by weighted size, internally sharded.
- `parking_lot::RwLock` is strictly better than `std::sync::RwLock` on macOS: no dynamic allocation, task-fair, up to 50x faster in multi-reader scenarios.
- `tokio::sync::RwLock` should NOT be used — blocking USB I/O must stay in `spawn_blocking`.
- `bytes::Bytes` enables zero-copy NFS read responses by slicing cached data without cloning.
- chrono replacement is low-priority — only 2 cold-path call sites. `time` crate is the right alternative.

# Performance & Reliability Optimization Implementation Plan

Created: 2026-04-08
Author: joshuareisbord@gmail.com
Status: COMPLETE
Approved: Yes
Iterations: 0
Worktree: No
Type: Feature

## Summary

**Goal:** Optimize fatx-rs for reliability and performance across all crates — I/O layer improvements, FAT allocation optimizations, concurrency model overhaul, FUSE-T mount migration, and dependency cleanup.

**Architecture:** Foundation-first approach: optimize fatxlib core (I/O sizing, FAT allocation, dirty tracking), then upgrade fatx-mount concurrency (RwLock, bounded caches, zero-copy), then spike and migrate to FUSE-T for structural reliability.

**Tech Stack:** Rust, `nix` (macOS ioctls/fcntl), `parking_lot` (fast sync primitives), `quick_cache` (bounded concurrent caches), `bytes` (zero-copy buffers), `time` (lightweight timestamps), FUSE-T embedded framework (libfuse-t C API)

**PRD:** `docs/prd/2026-04-08-performance-reliability-optimization.md`

## Scope

### In Scope

1. **fatxlib I/O layer** — configurable alignment (4KB for F_NOCACHE), macOS ioctls via `nix`, cluster-sized read buffers
2. **fatxlib FAT allocation** — `prev_free` hint, free-cluster bitmap, cached free count, dirty-range FAT tracking
3. **fatxlib dependencies** — replace `chrono` with `time`, add `nix`
4. **fatx-mount concurrency** — `parking_lot::RwLock` for volume, `quick_cache` for file/dir caches, `bytes::Bytes` for zero-copy
5. **fatx-mount FUSE-T** — spike to prove fuser+FUSE-T viability, then full migration replacing nfsserve
6. **fatx-mount dependency cleanup** — remove `nfsserve`, add `parking_lot`, `quick_cache`, `bytes`, `fuser`

### Out of Scope

- Streamed file I/O (cluster-chain-following reads) — separate future optimization
- Trait-based I/O backend (raw device vs file image divergence)
- TUI browser polish
- Windows/Linux support
- Contiguity detection / extent-based allocation
- Apple File Provider / FSKit
- NFS server fallback mode after FUSE-T migration

## Approach

**Chosen:** Foundation-first — optimize fatxlib core, then fatx-mount concurrency, then FUSE-T migration.

**Why:** fatxlib optimizations (I/O sizing, FAT allocation, dirty tracking) benefit ALL consumers (CLI, mount, mkimage) and are independent of the mount mechanism. Upgrading concurrency in fatx-mount before FUSE-T migration validates the patterns (RwLock, quick_cache, bytes) on the existing nfsserve architecture where we can compare before/after. The FUSE-T spike comes after the foundation is solid, so the new mount implementation starts with optimized primitives.

**Alternatives considered:**
- *FUSE-T spike first* — de-risks the unknown early but may duplicate concurrency work if FUSE-T changes the mount architecture. Rejected because fatxlib improvements are independent of mount choice.
- *Concurrency + FUSE-T together* — fewer iterations but larger individual tasks with more risk per task. Rejected because incremental verification is safer for an in-development codebase.

## Testing Philosophy

> **Physical drive verification first, then generated images, then automated tests.** The existing test suite may not be reliable. For each task:
> 1. Implement the change
> 2. Manually verify against the **known-good physical Xbox 360 drive** (1TB at `/dev/rdisk4` — verify device with `diskutil list`)
> 3. Once validated on the physical drive, verify against a **generated test image** (`fatx mkimage`)
> 4. THEN write/update automated tests that capture the verified behavior
>
> The physical drive is the ground truth. Do not rely on existing tests or generated images as the primary validation — those are verified second. Tests should be designed around behavior confirmed on real hardware.

## Context for Implementer

> This codebase is under active development. Core operations work but may be buggy or incomplete. Expect to encounter and fix issues in existing code during implementation.

- **Patterns to follow:** Endian-aware helpers in `volume.rs:280-310` (`read_u16`, `read_u32`, `write_u16_bytes`, `write_u32_bytes`). Sector-aligned I/O in `volume.rs:315-350` (`read_at`, `write_at`). NFS spawn_blocking pattern in `fatx-mount/src/main.rs:216-267`.
- **Conventions:** `fatxlib` uses `Result<T>` aliased to `Result<T, FatxError>`. Logging via `log` crate (`info!`, `warn!`, `debug!`). Tests in `fatxlib/tests/integration.rs` use `Cursor<Vec<u8>>` in-memory images.
- **Key files:**
  - `fatxlib/src/volume.rs` (1361 lines) — ALL core I/O, FAT operations, directory handling
  - `fatxlib/src/types.rs` — `DirectoryEntry`, `Superblock`, `FatEntry`, `FatType`, `FileAttributes`
  - `fatxlib/src/error.rs` — `FatxError` enum, `Result` type alias
  - `fatxlib/src/platform.rs` — macOS ioctl helpers (`get_block_device_size`)
  - `fatx-mount/src/main.rs` (~1900 lines) — NFS server, caching, write buffering, mount lifecycle
- **Gotchas:**
  - `read_at`/`write_at` allocate a new `Vec` on every call for sector alignment — this is the inner hot path
  - `allocate_cluster`/`allocate_chain` always scan from cluster 1 — O(n) per allocation
  - `flush()` writes the ENTIRE FAT cache (up to 231MB) even if one entry changed
  - `stats()` scans the entire FAT to count free clusters — O(n) every time
  - `FatxVolume` requires `&mut self` for ALL operations including reads (because `inner: T` needs `seek`+`read`) — this is why Mutex is used instead of RwLock currently. RwLock migration requires splitting read-only cache access from mutable device I/O.
  - NFS `read()` returns `Vec<u8>` — cloned from cache on every 128KB chunk
  - `write_file_in_place` has its own inline allocation loop (doesn't use `allocate_chain`) — must be updated when allocation changes
- **Domain context:** FATX is a simplified FAT variant. FAT16 uses 2-byte entries, FAT32 uses 4-byte entries. `fat_cache` is a flat byte array indexed by `cluster * entry_size`. The data area starts at `data_offset` (after superblock + FAT). Cluster numbering starts at 1 (`FIRST_CLUSTER`).

## Assumptions

- `nix` crate's `FcntlArg::F_NOCACHE` and `ioctl_read!` macro work correctly on macOS ARM64 — supported by nix 0.31.2 docs listing apple targets. Tasks 1-2 depend on this.
- `quick_cache::sync::Cache` supports weighted sizing by byte length for `Vec<u8>` / `Bytes` values — supported by quick_cache 0.6 docs. Task 7 depends on this.
- `fuser` crate works with FUSE-T's libfuse compatibility layer on macOS — UNVERIFIED, this is why Task 10 is a spike. Tasks 11-12 depend on this.
- **Existing tests may not be reliable.** The current test suite may not be testing the right things or working as intended. Implementation priority is: make it work (manual verification against real/generated images), THEN design tests around known-good behavior. Do not treat existing test pass/fail as ground truth.
- `parking_lot::RwLock` is a drop-in replacement for `std::sync::Mutex` when splitting read/write paths — supported by parking_lot 0.12 docs. Tasks 6-8 depend on this.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `fuser` doesn't work with FUSE-T on macOS | Medium | High | Task 10 is an isolated spike. If it fails, fall back to direct libfuse-t FFI bindings (Task 10 alt path). |
| F_NOCACHE 4KB alignment breaks existing Cursor-based tests | Low | Medium | Alignment constant is configurable per-volume (512 for Cursor tests, 4096 for raw devices). Task 1 parameterizes this. |
| RwLock for volume requires `&self` reads but FatxVolume needs `&mut self` | High | High | Refactor fat_cache-only methods (`read_fat_entry`, `read_chain`, `stats`) to `&self` in Task 7. Methods touching `inner` stay `&mut self`. Cache hits (majority) need no volume lock. Cache misses take write lock for device I/O. |
| Dirty-range FAT tracking introduces corruption on partial flush | Medium | Critical | Track dirty ranges as a `Vec<(start, end)>` merged into contiguous spans. Flush writes each span atomically. Regression test with mid-flush simulated failure. Task 4 addresses this. |
| FUSE-T embedded framework versioning/distribution | Low | Medium | Pin to a specific FUSE-T release. Include framework in repo or as build-time download. Task 11 addresses this. |
| FUSE-T installation blocked by macOS system extension policy | Medium | High | Task 10 spike starts with verifying FUSE-T installs and gets system extension approval. If blocked, pivot to improving nfsserve reliability instead of FUSE-T migration. |

## Goal Verification

### Truths

1. `cargo test --workspace` passes with 0 failures after all tasks complete
2. `allocate_cluster` on a half-full 500GB partition completes in <1ms (vs current O(n) scan)
3. `flush()` after modifying one FAT entry writes <8KB to disk (vs current 120-231MB)
4. `stats()` returns immediately from cached counts without scanning the FAT
5. NFS/FUSE concurrent reads from multiple Finder windows do not serialize on a global lock
6. fatx-mount process death results in automatic clean unmount (no stale mount, no Finder hang)
7. File transfer throughput via mounted volume is within 80% of raw USB bandwidth for large sequential files

### Artifacts

1. `fatxlib/src/volume.rs` — `prev_free`, `free_cluster_count`, `free_bitmap`, `dirty_ranges` fields; updated `allocate_cluster`, `flush`, `stats`
2. `fatxlib/src/platform.rs` — `configure_device_io` function with F_NOCACHE, ioctls
3. `fatx-mount/src/main.rs` or new `fatx-mount/src/fuse.rs` — FUSE filesystem implementation with `parking_lot::RwLock`, `quick_cache`, `bytes::Bytes`
4. `fatxlib/tests/integration.rs` — regression tests for dirty-range flush, prev_free allocation, bitmap accuracy

## Progress Tracking

- [x] Task 1: macOS I/O configuration via nix
- [x] Task 2: Configurable I/O alignment
- [x] Task 3: FAT allocation — prev_free hint + cached free count
- [x] Task 4: Dirty-range FAT tracking
- [x] Task 5: Free-cluster bitmap
- [x] Task 6: Replace chrono with time
- [x] Task 7: fatx-mount concurrency — RwLock + quick_cache + bytes
- [x] Task 8: fatx-mount dependency updates
- [x] Task 9: Verify fatx-mount with optimized fatxlib
- [x] Task 10: FUSE-T spike — DEFERRED (fuser + FUSE-T has friction: pkg-config shim, rpath issues, distribution complexity)
- [~] Task 11: FUSE-T mount implementation — DEFERRED (NFS approach retained)
- [~] Task 12: Remove nfsserve — DEFERRED (NFS approach retained)

**Total Tasks:** 12 | **Completed:** 10 | **Deferred:** 2 | **Remaining:** 0

## Implementation Tasks

### Task 1: macOS I/O Configuration via nix

**Objective:** Add macOS-native I/O configuration to fatxlib — set `F_NOCACHE` on raw device file descriptors to bypass kernel buffer cache, query `DKIOCGETPHYSICALBLOCKSIZE` and `DKIOCGETMAXBYTECOUNTREAD` to determine optimal I/O parameters for the specific USB device.

**Dependencies:** None

**Files:**

- Modify: `fatxlib/Cargo.toml` — add `nix` dependency with `fcntl`, `ioctl` features
- Modify: `fatxlib/src/platform.rs` — add `configure_device_io()`, ioctl queries using `nix` macros, replace raw `libc::ioctl` calls
- Modify: `fatxlib/src/volume.rs` — call platform configuration at open time, store results in `FatxVolume`
- Test: `fatxlib/tests/integration.rs` — test that Cursor-backed volumes skip device configuration gracefully

**Key Decisions / Notes:**

- Use `nix::fcntl::fcntl(fd, FcntlArg::F_NOCACHE(1))` — only when the inner type is `File` (not `Cursor` in tests)
- Also set `F_RDAHEAD(0)` to disable kernel read-ahead (useless for FATX since our app-level caches are more effective)
- Add `DeviceInfo` struct to platform.rs: `{ physical_block_size: u32, max_read_bytes: u64, io_saturation_bytes: u64 }`
- `FatxVolume` gets an optional `device_info: Option<DeviceInfo>` field — `None` for non-device backends
- The existing `get_block_device_size` in platform.rs already uses raw libc ioctls — refactor to use `nix::ioctl_read!` macro for type safety
- Guard all nix calls behind `#[cfg(target_os = "macos")]`

**Definition of Done:**

- [ ] `nix` dependency added to fatxlib/Cargo.toml
- [ ] `platform.rs` exports `configure_device_io(fd: RawFd) -> Option<DeviceInfo>` using nix macros
- [ ] `get_block_device_size` refactored to use nix ioctl macros
- [ ] `FatxVolume::open` calls `configure_device_io` when inner type provides a RawFd
- [ ] F_NOCACHE is set on raw device fds at open time
- [ ] F_RDAHEAD(0) is set on raw device fds at open time
- [ ] No diagnostics errors

**Verify:**

- `cargo build --workspace` succeeds
- Manual: `fatx mkimage .tmp/test.img --size 256M && fatx info .tmp/test.img` — verify DeviceInfo is `None` for file images
- Then write/update tests for the verified behavior

---

### Task 2: Configurable I/O Alignment

**Objective:** Replace hardcoded 512-byte sector alignment in `read_at`/`write_at` with configurable alignment that respects `F_NOCACHE` requirements (4096 bytes on macOS) and device physical block size.

**Dependencies:** Task 1

**Files:**

- Modify: `fatxlib/src/volume.rs` — add `alignment: u64` field to `FatxVolume`, update `open` (FAT loading alignment at ~line 188), `read_at`/`write_at`, and `flush` to use configurable alignment
- Test: `fatxlib/tests/integration.rs` — tests with both 512 and 4096 alignment, verify read_at/write_at correctness at various offsets

**Key Decisions / Notes:**

- Default alignment: 512 (backwards compatible for Cursor-backed tests and file images)
- When `DeviceInfo` is available: `alignment = max(512, physical_block_size, 4096)` — 4096 minimum when F_NOCACHE is active
- Replace `!0x1FF` mask with computed mask: `!(alignment - 1)`
- Replace `+ 511` rounding with `+ (alignment - 1)`
- **open() also uses hardcoded 512B alignment** at ~line 188 for FAT loading (`fat_aligned_start = fat_abs & !0x1FF`). Must update this too or FAT load fails with F_NOCACHE active.
- `read_at` currently allocates a new Vec on EVERY call — this is the hot path. Consider reusing a per-volume scratch buffer (but careful with borrow checker since `&mut self` is already held). At minimum, reduce allocations by reading cluster-sized chunks.
- `flush()` also uses hardcoded 512-byte alignment for FAT writes — must update

**Definition of Done:**

- [ ] `FatxVolume` has `alignment: u64` field, set from DeviceInfo or defaulting to 512
- [ ] `open()` FAT loading uses `self.alignment` (not hardcoded 0x1FF)
- [ ] `read_at` and `write_at` use `self.alignment` instead of hardcoded 0x1FF masks
- [ ] `flush()` uses `self.alignment` for FAT write alignment
- [ ] No diagnostics errors

**Verify:**

- `cargo build --workspace` succeeds
- Manual: `fatx mkimage .tmp/test.img --size 256M --populate && fatx ls .tmp/test.img /` — verify reads still work with new alignment logic
- Then write tests for 512 and 4096 alignment correctness

---

### Task 3: FAT Allocation — prev_free Hint + Cached Free Count

**Objective:** Add next-fit allocation via `prev_free` hint (matching Linux FAT driver pattern) and maintain `free_cluster_count` incrementally to eliminate O(n) scans in `stats()` and `allocate_cluster()`.

**Dependencies:** None (operates on fat_cache which is unchanged)

**Files:**

- Modify: `fatxlib/src/volume.rs` — add `prev_free: u32` and `free_cluster_count: u32` fields, update `open` to compute initial free count, update `allocate_cluster`/`allocate_chain` to use prev_free, update `write_fat_entry` to maintain free count, update `stats` to use cached count, update `write_file_in_place`'s inline allocation. Note: `VolumeStats` is defined in volume.rs (~line 1323), not types.rs.
- Test: `fatxlib/tests/integration.rs` — test that allocation advances prev_free, free count stays accurate through alloc/free cycles, stats matches manual scan

**Key Decisions / Notes:**

- `prev_free` initialized to `FIRST_CLUSTER` at open time
- `allocate_cluster` scans from `prev_free + 1`, wraps around to `FIRST_CLUSTER` if needed. Two-pass: first from prev_free to end, then from start to prev_free.
- `free_cluster_count` computed once at `open()` by scanning the FAT (same as current `stats()` does). After that, decremented on allocation, incremented on free.
- `write_fat_entry` must detect transitions: `Free -> !Free` (decrement count), `!Free -> Free` (increment count). Read old entry before writing new one.
- `stats()` becomes O(1) — just return the cached count. Keep `bad_clusters` and `used_clusters` as derived: `used = total - free - bad`. Bad clusters counted once at open (they don't change).
- `write_file_in_place` (volume.rs:942) has its own inline free cluster scan — must also use prev_free
- `allocate_chain` should allocate all clusters in one pass starting from prev_free
- **prev_free on free_chain:** Do NOT update prev_free when freeing clusters (match Linux FAT driver behavior). prev_free only advances on allocation. This keeps the implementation simple and avoids fragmentation from backward-seeking.

**Definition of Done:**

- [ ] `prev_free` and `free_cluster_count` fields added to `FatxVolume`
- [ ] `open()` computes initial free count and bad count by scanning FAT once
- [ ] `allocate_cluster` starts from `prev_free + 1` with wraparound
- [ ] `allocate_chain` starts from `prev_free + 1` with wraparound
- [ ] `write_file_in_place` inline allocation uses `prev_free`
- [ ] `write_fat_entry` maintains `free_cluster_count` on state transitions
- [ ] `stats()` returns cached counts in O(1) — no FAT scan
- [ ] No diagnostics errors

**Verify:**

- `cargo build --workspace` succeeds
- Manual: create a test image, write multiple files via CLI, verify `fatx stats` returns correct free space, verify allocation is fast on populated images
- Then write tests: free count accuracy after create/delete cycles, prev_free advances, stats matches manual FAT scan

---

### Task 4: Dirty-Range FAT Tracking

**Objective:** Replace all-or-nothing FAT flush with dirty-range tracking. Instead of writing the entire 120-231MB FAT on every `flush()`, track which byte ranges were modified and write only those.

**Dependencies:** Task 2 (needs configurable alignment for flush writes)

**Files:**

- Modify: `fatxlib/src/volume.rs` — add `dirty_ranges: Vec<(u64, u64)>` field (byte offsets within fat_cache), update `write_fat_entry` to record dirty range, update `flush` to write only dirty ranges, add `merge_dirty_ranges` helper
- Test: `fatxlib/tests/integration.rs` — test that flush after single write touches minimal bytes, test that multiple writes merge ranges, test corruption safety (all dirty ranges are flushed)

**Key Decisions / Notes:**

- Each `write_fat_entry` records the byte offset and length of the modified entry (2 or 4 bytes depending on FAT16/32)
- `merge_dirty_ranges`: sort by start, merge overlapping/adjacent ranges, then extend each to alignment boundary for the actual disk write
- `flush()`: iterate merged ranges, write each as an independent aligned I/O. **Remove each range from dirty_ranges immediately after successful write** — do NOT wait until all ranges succeed. This prevents stale data on retry: if range 3 fails, only range 3 remains dirty, and ranges 1-2 (already written) are not re-flushed with potentially stale data.
- Keep the existing `fat_dirty: bool` as a quick check — if false, skip flush entirely
- Safety: after iterating, if any ranges remain (write failed), leave `fat_dirty = true`. If all ranges were removed, set `fat_dirty = false`.
- For a typical operation (create one file): dirty range is ~4-8 bytes of FAT → one 4KB aligned write vs current 120MB write

**Definition of Done:**

- [ ] `dirty_ranges: Vec<(u64, u64)>` field added to `FatxVolume`
- [ ] `write_fat_entry` records dirty range on every write
- [ ] `flush()` writes only dirty ranges (aligned to `self.alignment`)
- [ ] `merge_dirty_ranges()` consolidates overlapping ranges
- [ ] After flush, dirty_ranges is cleared and fat_dirty is false
- [ ] No diagnostics errors

**Verify:**

- `cargo build --workspace` succeeds
- Manual: create test image, write one file via CLI with `--trace` logging, verify log shows dirty-range flush size (should be <8KB, not entire FAT). Write+delete cycle, verify image is not corrupted (read back all files).
- Then write tests: dirty-range sizes, create+delete flush correctness, no-op flush

---

### Task 5: Free-Cluster Bitmap

**Objective:** Build a secondary in-memory bitmap (1 bit per cluster) for O(1) free cluster lookup. For a 500GB partition, the bitmap is ~3.6MB vs scanning 120MB of FAT entries.

**Dependencies:** Task 3 (prev_free hint)

**Files:**

- Modify: `fatxlib/Cargo.toml` — add `bitvec` dependency (or use manual `Vec<u64>` bitmap)
- Modify: `fatxlib/src/volume.rs` — add `free_bitmap: Vec<u64>` field, build bitmap at `open()`, update `allocate_cluster`/`allocate_chain` to scan bitmap instead of FAT, update `write_fat_entry` to maintain bitmap on state transitions
- Test: `fatxlib/tests/integration.rs` — test bitmap accuracy vs FAT scan, test allocation from bitmap, test bitmap after free_chain

**Key Decisions / Notes:**

- Use `Vec<u64>` (manual bitmap, 64 clusters per word) rather than `bitvec` crate — simpler, no dependency, SIMD-friendly with `.trailing_zeros()` and `.count_ones()`
- Build at open time: iterate fat_cache, set bit for each free cluster. This adds negligible time to the existing FAT scan in open.
- `allocate_cluster`: find first set bit starting from `prev_free / 64` word. **In the starting word, mask out bits below `prev_free % 64`** before calling `trailing_zeros()` — otherwise `trailing_zeros()` may return a cluster before prev_free. Use `word & !((1u64 << (prev_free % 64)) - 1)` to mask. Clear the bit after allocation.
- `allocate_chain`: scan bitmap for `count` set bits, allocate all, then chain them in FAT.
- `write_fat_entry`: when transitioning to Free, set bit; when transitioning from Free, clear bit.
- Combined with prev_free (Task 3), allocation becomes: jump to prev_free word in bitmap, find first set bit, allocate. O(1) amortized for sequential allocation.

**Definition of Done:**

- [ ] `free_bitmap: Vec<u64>` field added to `FatxVolume`
- [ ] Bitmap built at `open()` from fat_cache
- [ ] `allocate_cluster` scans bitmap from prev_free word
- [ ] `allocate_chain` scans bitmap for multiple clusters
- [ ] `write_fat_entry` maintains bitmap on state transitions
- [ ] No diagnostics errors

**Verify:**

- `cargo build --workspace` succeeds
- Manual: create large test image (1G), populate with many files, verify allocation speed is noticeably faster, verify `fatx stats` still correct after writes
- Then write tests: bitmap matches FAT scan, allocate 1000 clusters with bitmap consistency, free_chain updates bitmap

---

### Task 6: Replace chrono with time

**Objective:** Replace the `chrono` dependency with the lighter `time` crate. Only 2 call sites in volume.rs use chrono for FAT timestamp encoding.

**Dependencies:** None

**Files:**

- Modify: `fatxlib/Cargo.toml` — remove `chrono`, add `time`
- Modify: `fatxlib/src/volume.rs` — replace `chrono::Utc::now()` with `time::OffsetDateTime::now_utc()` at both call sites (~line 831 and ~895)
- Test: `fatxlib/tests/integration.rs` — existing timestamp tests should still pass

**Key Decisions / Notes:**

- `chrono::Utc::now()` → `time::OffsetDateTime::now_utc()`
- Access year/month/day/hour/minute/second via `.year()`, `.month() as u8`, `.day()`, `.hour()`, `.minute()`, `.second()`
- Current code uses `now.format("%Y").to_string().parse().unwrap_or(2025)` — replace with direct `.year()` accessor (cleaner, no string formatting)
- `time` crate features needed: just default (no `formatting`, `parsing`, or `macros` needed)

**Definition of Done:**

- [ ] `chrono` removed from fatxlib/Cargo.toml
- [ ] `time` added to fatxlib/Cargo.toml
- [ ] Both timestamp call sites updated to use `time::OffsetDateTime::now_utc()`
- [ ] No more `format().parse().unwrap_or()` pattern — direct field access
- [ ] No diagnostics errors

**Verify:**

- `cargo build --workspace` succeeds
- Manual: create file on test image, verify timestamps are correct (inspect with `fatx ls -l`)
- Then verify existing timestamp tests still pass

---

### Task 7: fatx-mount Concurrency — RwLock + quick_cache + bytes

**Objective:** Overhaul the fatx-mount concurrency model: replace global `Mutex` with `parking_lot::RwLock`, replace unbounded `HashMap` caches with bounded `quick_cache::sync::Cache`, replace `Vec<u8>` file data with `bytes::Bytes` for zero-copy NFS responses.

**Dependencies:** Tasks 1-6 (fatxlib optimizations and chrono→time should be in place)

**Files:**

- Modify: `fatx-mount/Cargo.toml` — add `parking_lot`, `quick_cache`, `bytes`
- Modify: `fatx-mount/src/main.rs` — replace `FatxNfs` struct fields: `Mutex<FatxVolume>` → `RwLock<FatxVolume>`, `Mutex<HashMap<u32, Vec<DirectoryEntry>>>` → `Cache<u32, Vec<DirectoryEntry>>`, `Mutex<HashMap<u32, Vec<u8>>>` → `Cache<u32, Bytes>`, `Mutex<DirtyFileMap>` → `parking_lot::Mutex<DirtyFileMap>`, `Mutex<HashMap<u32, (u32, String)>>` → `RwLock<HashMap<u32, (u32, String)>>`
- Test: `fatx-mount/src/main.rs` — update existing tests for new types

**Key Decisions / Notes:**

- **Volume RwLock caveat and refactor:** `FatxVolume` requires `&mut self` for ALL operations (even reads) because `inner: T` needs seek+read. To make the RwLock meaningful, refactor fatxlib methods that only access `fat_cache` (like `read_fat_entry`, `read_chain`, `stats`) to take `&self` — the fat_cache is read-only after open (writes go through `write_fat_entry` which already needs `&mut self`). Methods that touch `inner` (read_at, write_at, read_cluster, write_cluster) stay `&mut self`. This lets NFS/FUSE reads that hit the FAT cache (not the device) use the read lock. Cache HITS from quick_cache need NO volume lock at all.
- **file_cache:** `Cache<u32, Bytes>` bounded by total byte weight. Configure with `with_weighter(256_MB, 1000, |_k, v: &Bytes| v.len())`. NFS read returns `cached.slice(offset..end)` — zero-copy.
- **dir_cache:** `Cache<u32, Vec<DirectoryEntry>>` bounded by entry count (1000 directories).
- **inode_parents:** `parking_lot::RwLock<HashMap<u32, (u32, String)>>` — small, read-heavy, no eviction needed.
- **dirty_files:** `parking_lot::Mutex<DirtyFileMap>` — write-only, short critical sections.
- **Lock ordering (prevent deadlocks):** (1) vol RwLock, (2) inode_parents RwLock, (3) file_cache/dir_cache (via quick_cache — no explicit lock), (4) dirty_files Mutex. Never acquire a higher-numbered lock while holding a lower-numbered one in reverse order.
- **NFS read hot path:** Check `file_cache.get(&cluster)` (no lock needed — quick_cache is concurrent) → if hit, slice and return. If miss, take vol write lock, read file, insert into cache as `Bytes::from(data)`.
- **invalidate_dir:** Use `cache.remove(&key)` on quick_cache — thread-safe, no lock needed.
- Update the periodic flush task to use `parking_lot::Mutex` for dirty_files.

**Definition of Done:**

- [ ] `parking_lot`, `quick_cache`, `bytes` added to fatx-mount/Cargo.toml
- [ ] fatxlib `read_fat_entry`, `read_chain`, `stats` refactored to `&self` (only access fat_cache, not inner)
- [ ] `FatxNfs` struct uses `RwLock<FatxVolume>`, `Cache<u32, Bytes>`, `Cache<u32, Vec<DirectoryEntry>>`, `parking_lot::Mutex<DirtyFileMap>`
- [ ] NFS `read()` returns `Bytes` slice from cache — zero-copy on cache hit
- [ ] NFS `write()` uses `parking_lot::Mutex` for dirty_files
- [ ] `invalidate_dir` uses quick_cache `remove()` — no Mutex
- [ ] flush task uses `parking_lot::Mutex`
- [ ] file_cache bounded to 256MB, dir_cache bounded to 1000 entries
- [ ] No diagnostics errors

**Verify:**

- `cargo build --workspace` succeeds
- Manual: mount test image via NFS, browse directories, copy files, verify cache hit logs, verify concurrent reads
- Then update fatx-mount tests for new types

---

### Task 8: fatx-mount Dependency Updates

**Objective:** Clean up fatx-mount dependencies — ensure `chrono` is removed if it was used directly, remove any now-unused dependencies, verify the dependency tree is minimal.

**Dependencies:** Task 7

**Files:**

- Modify: `fatx-mount/Cargo.toml` — audit and clean dependencies
- Modify: `Cargo.toml` (workspace root) — audit workspace dependencies

**Key Decisions / Notes:**

- Check if `fatx-mount` directly depends on `chrono` (it may use it for logging timestamps or NFS attr times)
- Check if `nfsserve` is still needed at this point (yes — FUSE-T migration hasn't happened yet)
- Run `cargo tree -p fatx-mount` to identify unused transitive deps
- This is a lightweight task — mainly verification and cleanup

**Definition of Done:**

- [ ] No unused direct dependencies in fatx-mount/Cargo.toml
- [ ] `cargo build --workspace` succeeds with no warnings about unused deps
- [ ] No diagnostics errors

**Verify:**

- `cargo build --workspace` succeeds
- `cargo tree -p fatx-mount` shows no unused deps

---

### Task 9: Verify fatx-mount with Optimized fatxlib

**Objective:** End-to-end verification that the optimized fatxlib works correctly with fatx-mount. Run the mount server against a test image and verify reads, writes, directory listing, and flush behavior.

**Dependencies:** Tasks 7, 8

**Files:**

- No file changes — this is a verification task
- Test: Manual verification with `fatx mkimage test.img --size 1G --populate && sudo fatx mount test.img --trace`

**Key Decisions / Notes:**

- Build release: `cargo build --release`
- Create test image: `fatx mkimage /tmp/test-opt.img --size 1G --populate`
- Mount and verify: basic read, write, directory listing, file copy, unmount
- Check logs for: dirty-range flush sizes, allocation from prev_free, cache hit rates
- **Concurrent write test (PRD Flow 4):** Copy 3+ directories with large files simultaneously via the mount. Verify: no deadlock, no ESTALE, no corruption, all files readable after transfer.
- **Throughput measurement:** Time a 100MB file copy through the mount. Compare informally against expectations for USB 3.0.
- This gates the FUSE-T migration — we need confidence the foundation is solid

**Definition of Done:**

- [ ] `cargo build --release` succeeds
- [ ] Test image mounts successfully via NFS
- [ ] Files can be read and written through the mount
- [ ] Logs show dirty-range flush (not full FAT flush)
- [ ] Logs show cache hits on repeated reads
- [ ] Concurrent multi-file copy completes without deadlock, ESTALE, or corruption
- [ ] Clean unmount with no errors
- [ ] No diagnostics errors

**Verify:**

- `cargo build --release && cargo test --workspace -q`

---

### Task 10: FUSE-T Spike — Prove fuser + FUSE-T on macOS

**Objective:** Minimal proof-of-concept that the `fuser` Rust crate works with FUSE-T's libfuse compatibility layer on macOS. Mount a trivial read-only filesystem (hardcoded files) via FUSE-T to prove the toolchain works.

**Dependencies:** None (can run in parallel with Tasks 1-9, but placed here in sequence for implementation order)

**Files:**

- Create: `.tmp/fuse-spike/` — temporary spike directory (gitignored)
- Create: `.tmp/fuse-spike/Cargo.toml` — minimal binary with `fuser` dependency
- Create: `.tmp/fuse-spike/src/main.rs` — trivial FUSE filesystem (getattr + readdir + read for one hardcoded file)

**Key Decisions / Notes:**

- **Step 0: Verify FUSE-T installation FIRST.** FUSE-T may require System Settings > Privacy & Security approval on macOS Sequoia+, and possibly a reboot. Run `brew install fuse-t` and verify `/Library/Frameworks/fuse-t.framework` exists before writing any code. If installation is blocked by system extension policy, document the issue and pivot to direct NFS improvements instead.
- The spike tests: (1) FUSE-T is installed and framework exists, (2) `fuser` compiles on macOS, (3) FUSE-T's libfuse headers are found, (4) a trivial filesystem mounts, (5) `ls` and `cat` work on the mounted filesystem, (6) process kill results in clean unmount
- If fuser+FUSE-T works: proceed to Task 11 with fuser
- If fuser fails: pivot to direct libfuse-t C FFI bindings (document the failure and alternative approach)
- Delete `.tmp/fuse-spike/` after spike is complete regardless of outcome

**Definition of Done:**

- [ ] Spike binary compiles with fuser on macOS
- [ ] FUSE-T mount succeeds — trivial filesystem visible in Finder
- [ ] `ls` and `cat` work on mounted files
- [ ] Process kill results in clean unmount (FUSE-T auto-unmount)
- [ ] Spike results documented (works / doesn't work + why)
- [ ] `.tmp/fuse-spike/` deleted after completion

**Verify:**

- Manual: mount spike, `ls /tmp/fuse-spike-mount`, kill process, verify unmount

---

### Task 11: FUSE-T Mount Implementation

**Objective:** Implement a FUSE filesystem in fatx-mount that replaces the nfsserve NFS server. The FUSE implementation uses the same optimized fatxlib and the concurrency patterns from Task 7 (RwLock, quick_cache, bytes).

**Dependencies:** Tasks 7, 9, 10 (spike must succeed)

**Files:**

- Create: `fatx-mount/src/fuse.rs` — FUSE filesystem implementation: `FatxFuse` struct implementing `fuser::Filesystem` trait
- Modify: `fatx-mount/src/main.rs` — add `--fuse` flag (default), keep `--nfs` for backwards compat during transition, wire up FUSE mount path
- Modify: `fatx-mount/Cargo.toml` — add `fuser` dependency

**Key Decisions / Notes:**

- `FatxFuse` struct mirrors `FatxNfs` fields: `RwLock<FatxVolume>`, `Cache` for file/dir, `Mutex<DirtyFileMap>`, etc.
- Implement FUSE ops: `getattr`, `readdir`, `read`, `write`, `create`, `mkdir`, `unlink`, `rmdir`, `rename`, `statfs`, `flush`, `release`
- fuser uses blocking trait methods (not async) — this is actually simpler than the current tokio+nfsserve approach. No `spawn_blocking` needed; fuser manages its own thread pool.
- Mount via FUSE-T embedded framework — link against `/Library/Frameworks/fuse-t.framework` or detect at build time
- FUSE-T auto-unmount on process death eliminates the need for the complex signal handler / panic handler / watchdog infrastructure
- Keep the periodic flush task (dirty file writes every 5s) — this is independent of mount mechanism
- **FUSE flush/release integration:** FUSE's `flush()` callback is called on `close(2)`. It MUST trigger an immediate flush of that file's dirty data to disk (don't wait for the 5s timer). FUSE's `release()` (last close) should also flush. If a user saves a file and immediately reads it back, the data must come from dirty_files before the timer fires.
- **statfs must use cached free count** from Task 3 — macOS calls statfs frequently (Finder refresh, `df`). Without cached count, every statfs triggers a full FAT scan.
- inode mapping: FUSE uses `u64` inodes, same as current NFS fileid mapping (cluster number as inode)

**Definition of Done:**

- [ ] `fuse.rs` implements `fuser::Filesystem` with all required ops
- [ ] `--fuse` flag mounts via FUSE-T
- [ ] Files can be read, written, created, deleted, renamed through FUSE mount
- [ ] Directory browsing works in Finder
- [ ] Process kill results in clean auto-unmount (FUSE-T)
- [ ] `statfs` uses cached free count (O(1), no FAT scan)
- [ ] FUSE `flush()`/`release()` triggers immediate write of dirty file data to disk
- [ ] Periodic flush task works with FUSE mount
- [ ] File cache uses `Bytes` for zero-copy reads
- [ ] All existing fatx-mount tests pass
- [ ] No diagnostics errors

**Verify:**

- `cargo test --workspace -q`
- Manual: mount test image via `--fuse`, browse in Finder, copy files, kill process, verify auto-unmount

---

### Task 12: Remove nfsserve, Finalize FUSE-T Migration

**Objective:** Remove the nfsserve NFS server code, make FUSE-T the sole mount mechanism, clean up dead code (NFS-specific signal handlers, panic handlers, watchdog), update CLI help text.

**Dependencies:** Task 11

**Files:**

- Modify: `fatx-mount/Cargo.toml` — remove `nfsserve` dependency
- Modify: `fatx-mount/src/main.rs` — remove `FatxNfs` struct and all NFS-related code, remove `--nfs` flag, simplify signal/panic handlers (FUSE-T handles unmount), remove NFS-specific mount options
- Delete: Any NFS-specific helper functions no longer needed
- Modify: `CLAUDE.md` — update NFS mount documentation to reflect FUSE-T

**Key Decisions / Notes:**

- This is a cleanup task — all functionality should already work from Task 11
- Remove: `FatxNfs` struct, `impl NFSFileSystem for FatxNfs`, NFS mount command construction, NFS-specific mount options (`soft,intr,retrans,timeo`)
- **Simplify but keep `--cleanup`:** FUSE-T auto-unmounts on process death, but edge cases exist (macOS kernel bug, power loss, FUSE-T framework crash) that can leave stale mountpoint directories. Keep a simplified `--cleanup` that removes stale mountpoint directories — no longer needs NFS-specific cleanup.
- Keep: periodic flush task, dirty file management, cache infrastructure (now used by FUSE impl)
- Update CLAUDE.md section on NFS mount to reflect FUSE-T architecture
- The complex signal handler infrastructure (dedicated thread, panic hook, emergency unmount) can be greatly simplified — FUSE-T handles unmount automatically

**Definition of Done:**

- [ ] `nfsserve` removed from Cargo.toml
- [ ] All NFS-specific code removed from main.rs
- [ ] `FatxNfs` struct and `impl NFSFileSystem` removed
- [ ] Signal/panic handlers simplified (FUSE-T auto-unmount)
- [ ] `--cleanup` simplified to remove stale mountpoint directories only (no NFS cleanup)
- [ ] CLAUDE.md updated
- [ ] `cargo build --workspace` compiles with no dead code warnings
- [ ] All tests pass
- [ ] No diagnostics errors

**Verify:**

- `cargo test --workspace -q`
- `cargo build --release`

## Open Questions

1. **FUSE-T embedded framework build integration:** How to link against the embedded framework at build time? Cargo build script (`build.rs`) with `println!("cargo:rustc-link-search=framework=...")` or pkg-config? This will be resolved during the FUSE-T spike (Task 10).

2. **fuser macOS compatibility:** fuser lists macOS as "untested." The spike (Task 10) will determine if it works with FUSE-T or if direct FFI is needed.

3. **Cache sizing:** The 256MB file cache and 1000-entry dir cache are starting points. May need tuning based on real-world usage with large game transfers. Can be made configurable via CLI flags later.

### Deferred Ideas

- **FUSE-T mount migration (Tasks 11-12):** Spike (Task 10) found that fuser + FUSE-T has significant integration friction: requires a fake `osxfuse.pc` pkg-config shim with version spoofing, runtime `libfuse-t.dylib` not found without rpath hacks, and distribution would require FUSE-T installed on user machines. The NFS approach is retained — it works well, has no external dependencies, and was validated on real hardware. Revisit when fuser adds native FUSE-T support or FUSE-T provides macFUSE-compatible pkg-config.
- **Streamed file I/O:** Read files in cluster-sized chunks instead of loading entire file into Vec<u8>. Would reduce peak memory for 4GB game files. Deferred because the caching layer handles NFS chunking adequately.
- **Contiguity detection:** Track whether allocated clusters are contiguous and write entire files in a single I/O when possible. Deferred because it requires restructuring the allocation API.
- **FSKit backend:** FUSE-T supports FSKit on macOS 26+ (Tahoe). When adoption is sufficient, this becomes the cleanest path to a native filesystem experience.
- **CLI `--cache-size` flag:** Allow users to configure file cache and dir cache sizes via command-line flags.

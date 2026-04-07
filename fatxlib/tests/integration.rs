//! Integration tests for fatxlib.
//!
//! These tests create an in-memory FATX image, format it, and exercise
//! the full read/write API.

use std::io::Cursor;

use fatxlib::types::*;
use fatxlib::volume::FatxVolume;

/// Create a minimal FATX image in memory.
/// Returns a Cursor wrapping the image bytes.
fn create_test_image(size_mb: usize) -> Cursor<Vec<u8>> {
    let size = size_mb * 1024 * 1024;
    let mut data = vec![0u8; size];

    // Write superblock
    data[0] = b'F';
    data[1] = b'A';
    data[2] = b'T';
    data[3] = b'X';
    // Volume ID
    data[4..8].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
    // Sectors per cluster = 32 (16 KB clusters)
    data[8..12].copy_from_slice(&32u32.to_le_bytes());
    // FAT copies = 1
    data[12..14].copy_from_slice(&1u16.to_le_bytes());

    // We need to initialize the FAT area:
    // After superblock (0x1000), the FAT begins.
    // For a small image, we use FAT16.
    // Cluster count ~ (size - 4096) / (16384 + 2)
    // For 2MB: ~121 clusters => FAT16
    // FAT entries start at 0x1000.
    //
    // Mark cluster 1 (root directory) as end-of-chain.
    // Cluster 1 FAT entry is at offset 0x1000 + 1*2 = 0x1002
    let fat_offset = 0x1000usize;
    // Cluster 1 = root dir, mark as EOC (0xFFF8)
    data[fat_offset + 2] = 0xF8;
    data[fat_offset + 3] = 0xFF;

    // Initialize the root directory cluster with end markers (0xFF)
    // We need to know where data starts. FAT size = clusters * 2, rounded to 4KB.
    // For 2MB: ~121 clusters * 2 = 242 bytes, rounded to 4096.
    // Data starts at 0x1000 + 0x1000 = 0x2000
    // Cluster 1 data is at 0x2000 + (1-1) * 16384 = 0x2000
    let data_offset = 0x2000usize;
    // Fill root directory cluster with 0xFF (end-of-directory markers)
    for i in 0..16384 {
        if data_offset + i < data.len() {
            data[data_offset + i] = 0xFF;
        }
    }

    Cursor::new(data)
}

#[test]
fn test_open_volume() {
    let cursor = create_test_image(2);
    let vol = FatxVolume::open(cursor, 0, 0).expect("Failed to open volume");
    assert!(vol.superblock.is_valid());
    assert_eq!(vol.superblock.volume_id, 0xDEADBEEF);
    assert_eq!(vol.superblock.sectors_per_cluster, 32);
    assert_eq!(vol.fat_type, FatType::Fat16);
}

#[test]
fn test_read_empty_root() {
    let cursor = create_test_image(2);
    let mut vol = FatxVolume::open(cursor, 0, 0).expect("Failed to open volume");
    let entries = vol.read_root_directory().expect("Failed to read root dir");
    assert!(entries.is_empty(), "Root directory should be empty");
}

#[test]
fn test_create_and_read_file() {
    let cursor = create_test_image(2);
    let mut vol = FatxVolume::open(cursor, 0, 0).expect("Failed to open volume");

    let test_data = b"Hello, Xbox FATX filesystem!";
    vol.create_file("/test.txt", test_data)
        .expect("Failed to create file");

    // Read it back
    let read_data = vol
        .read_file_by_path("/test.txt")
        .expect("Failed to read file");
    assert_eq!(read_data, test_data);
}

#[test]
fn test_create_directory_and_file_inside() {
    let cursor = create_test_image(2);
    let mut vol = FatxVolume::open(cursor, 0, 0).expect("Failed to open volume");

    vol.create_directory("/saves")
        .expect("Failed to create directory");

    // Verify directory appears in root
    let root_entries = vol.read_root_directory().expect("Failed to read root");
    assert_eq!(root_entries.len(), 1);
    assert!(root_entries[0].is_directory());
    assert_eq!(root_entries[0].filename(), "saves");

    // Create a file inside the directory
    let save_data = b"save game data here";
    vol.create_file("/saves/game1.sav", save_data)
        .expect("Failed to create file in subdir");

    // Read it back
    let read_data = vol
        .read_file_by_path("/saves/game1.sav")
        .expect("Failed to read file from subdir");
    assert_eq!(read_data, save_data);
}

#[test]
fn test_delete_file() {
    let cursor = create_test_image(2);
    let mut vol = FatxVolume::open(cursor, 0, 0).expect("Failed to open volume");

    vol.create_file("/deleteme.txt", b"temporary data")
        .expect("Failed to create file");

    // Verify it exists
    let entries = vol.read_root_directory().expect("read root");
    assert_eq!(entries.len(), 1);

    // Delete it
    vol.delete("/deleteme.txt").expect("Failed to delete");

    // Verify it's gone
    let entries = vol.read_root_directory().expect("read root");
    assert_eq!(entries.len(), 0);
}

#[test]
fn test_rename_file() {
    let cursor = create_test_image(2);
    let mut vol = FatxVolume::open(cursor, 0, 0).expect("Failed to open volume");

    vol.create_file("/old.txt", b"some data")
        .expect("Failed to create file");

    vol.rename("/old.txt", "new.txt")
        .expect("Failed to rename");

    // Old name should not resolve
    assert!(vol.resolve_path("/old.txt").is_err());

    // New name should work and data should be intact
    let data = vol
        .read_file_by_path("/new.txt")
        .expect("Failed to read renamed file");
    assert_eq!(data, b"some data");
}

#[test]
fn test_volume_stats() {
    let cursor = create_test_image(2);
    let mut vol = FatxVolume::open(cursor, 0, 0).expect("Failed to open volume");

    let stats = vol.stats().expect("Failed to get stats");
    assert!(stats.total_clusters > 0);
    // Root directory uses 1 cluster, everything else should be free
    assert!(stats.free_clusters > 0);
    assert_eq!(stats.bad_clusters, 0);
}

#[test]
fn test_filename_validation() {
    let cursor = create_test_image(2);
    let mut vol = FatxVolume::open(cursor, 0, 0).expect("Failed to open volume");

    // Filename too long (>42 chars)
    let long_name = "/".to_string() + &"a".repeat(50);
    let result = vol.create_file(&long_name, b"data");
    assert!(result.is_err());
}

#[test]
fn test_directory_entry_timestamps() {
    // Test date encoding/decoding round-trip
    let date = DirectoryEntry::encode_date(2024, 3, 15);
    let (y, m, d) = DirectoryEntry::decode_date(date);
    assert_eq!((y, m, d), (2024, 3, 15));

    let time = DirectoryEntry::encode_time(14, 30, 22);
    let (h, min, s) = DirectoryEntry::decode_time(time);
    assert_eq!((h, min), (14, 30));
    // Seconds have 2-second resolution
    assert_eq!(s, 22);
}

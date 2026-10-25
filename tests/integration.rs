use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use ibps::{
    files,
    patch::{self, Format},
};

const MIB: usize = 1024 * 1024;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ibps-integration-{}-{}-{label}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn generated(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn assert_round_trip(format: Format, source: &[u8], target: &[u8]) {
    let created = patch::create(format, source, target).unwrap();
    assert_eq!(patch::identify(&created.data), Some(format));
    assert_eq!(patch::apply(&created.data, source).unwrap().data, target);
}

#[test]
fn all_formats_round_trip_same_length_and_growth() {
    let source = generated(4096, 0x1234_5678_9abc_def0);
    let mut same_length = source.clone();
    same_length[0] ^= 0x5a;
    same_length[127..191].fill(0xe7);
    same_length[2048..2304].copy_from_slice(&generated(256, 0x55aa));

    let mut grown = source.clone();
    grown.extend(generated(777, 0xfedc_ba98_7654_3210));
    grown[4095] ^= 1;

    for format in [Format::Ips, Format::Ips32, Format::Ups, Format::Bps] {
        assert_round_trip(format, &source, &same_length);
        assert_round_trip(format, &source, &grown);
    }
}

#[test]
fn shrinking_round_trips_except_for_ips32() {
    let source = generated(3072, 0x1020_3040_5060_7080);
    let mut target = source[..913].to_vec();
    target[17..41].fill(0x3c);

    for format in [Format::Ips, Format::Ups, Format::Bps] {
        assert_round_trip(format, &source, &target);
    }
    let error = patch::create(Format::Ips32, &source, &target).unwrap_err();
    assert!(error.contains("小さ"), "unexpected error: {error}");
}

#[test]
fn deterministic_generated_cases_round_trip() {
    for (case, (source_len, target_len)) in [(0, 0), (1, 1), (31, 97), (257, 257), (1024, 733)]
        .into_iter()
        .enumerate()
    {
        let source = generated(source_len, 0xa5a5_0000 + case as u64);
        let mut target = generated(target_len, 0x5a5a_0000 + case as u64);
        if source_len == target_len && source_len > 2 {
            target[..source_len / 2].copy_from_slice(&source[..source_len / 2]);
        }
        for format in [Format::Ips, Format::Ups, Format::Bps] {
            assert_round_trip(format, &source, &target);
        }
        if target_len >= source_len {
            assert_round_trip(Format::Ips32, &source, &target);
        }
    }
}

fn expansion_case(format: Format, original_size: usize, expanded_size: usize, fill: u8) {
    let original = generated(original_size, 0x3141_5926_5358_9793);
    let mut expanded_source = original.clone();
    expanded_source.resize(expanded_size, fill);
    let mut target = expanded_source.clone();
    target[original_size + 31] ^= 0x81;
    target[expanded_size - 4097..expanded_size - 4001].fill(0x6d);
    target[expanded_size - 1] ^= 1;

    let created = patch::create(format, &expanded_source, &target).unwrap();
    assert_eq!(patch::apply(&created.data, &original).unwrap().data, target);
}

/// Exercises 4-to-8 MiB IPS growth and 16-to-32 MiB IPS32, UPS, and BPS growth.
/// Run with: cargo test --release --test integration large_rom_expansion -- --ignored
#[test]
#[ignore = "large ROM and BPS suffix-array test; run explicitly in release mode"]
fn large_rom_expansion_matches_design() {
    expansion_case(Format::Ips, 4 * MIB, 8 * MIB, 0xff);
    for format in [Format::Ips32, Format::Ups, Format::Bps] {
        expansion_case(format, 16 * MIB, 32 * MIB, 0xff);
    }
    for format in [Format::Ups, Format::Bps] {
        expansion_case(format, 16 * MIB, 32 * MIB, 0x00);
    }
}

#[test]
fn applying_over_the_input_is_an_atomic_replacement() {
    let dir = TempDir::new("replace-input");
    let patch_path = dir.join("更新.ips");
    let input_path = dir.join("ゲーム本体.gba");
    let source = generated(2048, 11);
    let mut target = source.clone();
    target[12..100].fill(0xcc);
    fs::write(
        &patch_path,
        patch::create(Format::Ips, &source, &target).unwrap().data,
    )
    .unwrap();
    fs::write(&input_path, &source).unwrap();

    files::apply_files(&patch_path, &input_path, Some(&input_path)).unwrap();

    assert_eq!(fs::read(input_path).unwrap(), target);
}

#[test]
fn invalid_patch_preserves_an_existing_output() {
    let dir = TempDir::new("preserve-output");
    let patch_path = dir.join("破損.ups");
    let input_path = dir.join("入力.bin");
    let output_path = dir.join("既存の出力.bin");
    let sentinel = b"must remain untouched";
    fs::write(&patch_path, b"UPS1broken").unwrap();
    fs::write(&input_path, generated(128, 12)).unwrap();
    fs::write(&output_path, sentinel).unwrap();

    assert!(files::apply_files(&patch_path, &input_path, Some(&output_path)).is_err());

    assert_eq!(fs::read(output_path).unwrap(), sentinel);
}

#[test]
fn implicit_default_output_refuses_a_conflicting_file() {
    let dir = TempDir::new("default-conflict");
    let patch_path = dir.join("配布パッチ.ips");
    let input_path = dir.join("原作.gba");
    let default_output = dir.join("配布パッチ.gba");
    let source = generated(256, 13);
    let mut target = source.clone();
    target[3] ^= 0xff;
    fs::write(
        &patch_path,
        patch::create(Format::Ips, &source, &target).unwrap().data,
    )
    .unwrap();
    fs::write(&input_path, &source).unwrap();
    fs::write(&default_output, b"existing").unwrap();

    let error = files::apply_files(&patch_path, &input_path, None).unwrap_err();

    assert!(error.contains("すでに存在"), "unexpected error: {error}");
    assert_eq!(fs::read(default_output).unwrap(), b"existing");
}

#[test]
fn japanese_paths_work_for_creation_and_application() {
    let dir = TempDir::new("日本語パス");
    let source_path = dir.join("変更前データ.bin");
    let target_path = dir.join("変更後データ.bin");
    let patch_path = dir.join("差分パッチ.bps");
    let output_path = dir.join("適用結果.bin");
    let source = generated(1024, 14);
    let mut target = source.clone();
    target.splice(300..340, generated(91, 15));
    fs::write(&source_path, &source).unwrap();
    fs::write(&target_path, &target).unwrap();

    files::create_files(Format::Bps, &source_path, &target_path, &patch_path).unwrap();
    files::apply_files(&patch_path, &source_path, Some(&output_path)).unwrap();

    assert_eq!(fs::read(output_path).unwrap(), target);
}

#[test]
fn creation_rejects_the_same_canonical_path() {
    let dir = TempDir::new("canonical");
    let source_path = dir.join("同じファイル.bin");
    let equivalent_path = dir.join(".").join("同じファイル.bin");
    let patch_path = dir.join("作成されない.ips");
    fs::write(&source_path, generated(32, 16)).unwrap();

    let error =
        files::create_files(Format::Ips, &source_path, &equivalent_path, &patch_path).unwrap_err();

    assert!(error.contains("同じパス"), "unexpected error: {error}");
    assert!(!patch_path.exists());
}

#[test]
fn cli_reports_results_errors_and_default_paths() {
    let dir = TempDir::new("cli-日本語");
    let source = dir.join("変更前.gba");
    let target = dir.join("変更後.gba");
    let patch = dir.join("変更後.bps");
    let out = dir.join("適用結果.gba");
    fs::write(&source, b"source data source data").unwrap();
    fs::write(&target, b"target data target data").unwrap();
    let run = |args: &[&OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_ibps"))
            .args(args)
            .output()
            .unwrap()
    };
    let result = run(&[
        OsStr::new("--create"),
        OsStr::new("--bps"),
        source.as_os_str(),
        target.as_os_str(),
    ]);
    assert!(result.status.success());
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("パッチを作成しました")
    );
    assert!(result.stderr.is_empty());
    assert!(patch.exists());
    let result = run(&[OsStr::new("--info"), patch.as_os_str()]);
    assert!(result.status.success());
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("変更前CRC32")
    );
    let result = run(&[OsStr::new("--apply"), patch.as_os_str(), source.as_os_str()]);
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("すでに存在")
    );
    let result = run(&[
        OsStr::new("--apply"),
        patch.as_os_str(),
        source.as_os_str(),
        out.as_os_str(),
    ]);
    assert!(result.status.success());
    assert_eq!(fs::read(&out).unwrap(), fs::read(&target).unwrap());
    let result = run(&[
        OsStr::new("--apply"),
        patch.as_os_str(),
        out.as_os_str(),
        out.as_os_str(),
    ]);
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("適用済み")
    );
    assert_eq!(fs::read(out).unwrap(), fs::read(target).unwrap());
}

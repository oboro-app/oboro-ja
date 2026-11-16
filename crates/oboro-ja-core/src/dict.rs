//! 形態素解析辞書(SudachiDict、lindera 形式)の検証と取得(設計書 §6.5)。
//!
//! 辞書の在り処は呼ぶ側が決める。この module は path を引数で受け取り、
//! 環境変数・cwd・既定の置き場を探さない。配布物のダウンロードも呼ぶ側が行う。

use std::fs;
use std::io::{self, Seek as _};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use sha2::{Digest as _, Sha256};

/// lindera のファイル辞書ローダが実際に読み込むファイル集合の正本(#36)。
/// `scripts/package-dict.sh` の必須ファイル検査と同じ集合に保つ。
/// 外部辞書の内容識別はこの集合を対象にし、sys.dic のような
/// 別形式の固定ファイル名には依存しない
pub const DICTIONARY_FILES: [&str; 9] = [
    "char_def.bin",
    "dict.trie",
    "dict.vals",
    "dict.valsidx",
    "dict.words",
    "dict.wordsidx",
    "matrix.mtx",
    "metadata.json",
    "unk.bin",
];

/// 検証済みの辞書ディレクトリ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDictionary {
    /// 辞書ディレクトリ
    pub dir: PathBuf,
    /// 内容 fingerprint(`manifest_sha256` の値)
    pub manifest_sha256: String,
}

/// 辞書ディレクトリが Sudachi 形式であることを確認する(#98)。
/// `file://` URI でも指定できるため剥いでから検査する
pub fn ensure_sudachi_format(path: &str) -> Result<()> {
    let fs_path = path.strip_prefix("file://").unwrap_or(path);
    let metadata = Path::new(fs_path).join("metadata.json");
    ensure!(
        metadata.is_file(),
        "辞書ディレクトリに metadata.json がありません。jaio は Sudachi 形式の辞書(lindera-sudachidict で展開されたもの)にのみ対応しています: {}",
        fs_path
    );
    Ok(())
}

/// 外部辞書ディレクトリの内容 fingerprint を返す。
/// lindera が実際にロードするファイル集合(`DICTIONARY_FILES`)の
/// 「名前・バイト長・SHA-256」のマニフェストをさらに SHA-256 した
/// 決定的な識別子。大きい辞書に備えてファイル内容はストリーミングで読む
pub fn manifest_sha256(dir: &Path) -> Result<String> {
    let mut manifest = String::new();
    for name in DICTIONARY_FILES {
        let file_path = dir.join(name);
        let mut file = fs::File::open(&file_path)
            .with_context(|| format!("辞書ファイルを開けません: {}", file_path.display()))?;
        let mut hasher = Sha256::new();
        let len = io::copy(&mut file, &mut hasher)
            .with_context(|| format!("辞書ファイルを読めません: {}", file_path.display()))?;
        manifest.push_str(&format!("{name}\t{len}\t{:x}\n", hasher.finalize()));
    }
    Ok(format!("{:x}", Sha256::digest(manifest.as_bytes())))
}

/// 辞書ディレクトリを検証する。lindera が読む 9 ファイルが揃っていることを
/// 確かめ、内容 fingerprint を返す
pub fn verify_dir(dir: &Path) -> Result<VerifiedDictionary> {
    let missing: Vec<&str> = DICTIONARY_FILES
        .iter()
        .copied()
        .filter(|name| !dir.join(name).is_file())
        .collect();
    ensure!(
        missing.is_empty(),
        "辞書ディレクトリに必要なファイルがありません(欠落: {}): {}",
        missing.join(", "),
        dir.display()
    );
    Ok(VerifiedDictionary {
        dir: dir.to_path_buf(),
        manifest_sha256: manifest_sha256(dir)?,
    })
}

/// SHA-256 の期待値を取り出す。64 桁の 16 進数そのものと、`shasum` /
/// `sha256sum` の出力(`<hex>  <ファイル名>`。`scripts/package-dict.sh` が
/// 作る `.sha256` の内容)の両方を受け付ける
pub fn parse_sha256(text: &str) -> Result<String> {
    let token = text.split_whitespace().next().unwrap_or("");
    ensure!(
        token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
        "SHA-256 の値が不正です(64 桁の 16 進数が必要です)"
    );
    Ok(token.to_ascii_lowercase())
}

/// 配布物(tar.gz)の SHA-256 を期待値と照合する
pub fn verify_archive_sha256(archive: &Path, expected: &str) -> Result<()> {
    let expected = parse_sha256(expected)?;
    let mut file = fs::File::open(archive)
        .with_context(|| format!("配布物を開けません: {}", archive.display()))?;
    ensure_stream_sha256(&mut file, &expected, archive)
}

/// 配布物(`scripts/package-dict.sh` が作る tar.gz)を照合してから展開し、
/// `dest` に辞書ディレクトリとして置く。
///
/// - SHA-256 が一致しない配布物は展開しない
/// - 展開するのは `DICTIONARY_FILES` の通常ファイルだけ。それ以外の項目は
///   書き出さない。9 ファイルが 1 つのディレクトリに揃っていない配布物は置かない
/// - `dest` が既にあるときは上書きせずエラーにする
/// - 作業ディレクトリへ展開して検証を終えてから `dest` へ rename するため、
///   失敗時に `dest` へ不完全な辞書が残らない
pub fn install_archive(
    archive: &Path,
    expected_sha256: &str,
    dest: &Path,
) -> Result<VerifiedDictionary> {
    let expected = parse_sha256(expected_sha256)?;
    ensure!(
        dest.symlink_metadata().is_err(),
        "置き先が既に存在します: {}",
        dest.display()
    );
    let Some(name) = dest.file_name() else {
        bail!("置き先のディレクトリ名を決められません: {}", dest.display());
    };
    let parent = match dest.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };

    // 照合と展開は同じファイルハンドルで行う(照合の後に path の指す
    // 実体を差し替えられても、展開する内容は照合したものと変わらない)
    let mut file = fs::File::open(archive)
        .with_context(|| format!("配布物を開けません: {}", archive.display()))?;
    ensure_stream_sha256(&mut file, &expected, archive)?;
    file.rewind()
        .with_context(|| format!("配布物を読み直せません: {}", archive.display()))?;

    fs::create_dir_all(parent)
        .with_context(|| format!("置き先の親ディレクトリを作れません: {}", parent.display()))?;
    // rename を同じファイルシステムの中に保つため、作業ディレクトリは
    // 置き先と同じ親に作る
    let mut staging_name = std::ffi::OsString::from(".");
    staging_name.push(name);
    staging_name.push(format!(".partial-{}", std::process::id()));
    let staging = parent.join(staging_name);
    if staging.symlink_metadata().is_ok() {
        fs::remove_dir_all(&staging)
            .with_context(|| format!("古い作業ディレクトリを消せません: {}", staging.display()))?;
    }
    fs::create_dir(&staging)
        .with_context(|| format!("作業ディレクトリを作れません: {}", staging.display()))?;

    let installed = extract_dictionary_files(file, &staging)
        .with_context(|| format!("配布物を展開できません: {}", archive.display()))
        .and_then(|()| verify_dir(&staging))
        .and_then(|verified| {
            fs::rename(&staging, dest)
                .with_context(|| format!("辞書を置けません: {}", dest.display()))?;
            Ok(VerifiedDictionary {
                dir: dest.to_path_buf(),
                manifest_sha256: verified.manifest_sha256,
            })
        });
    if installed.is_err() {
        // 失敗の原因を優先して返す。作業ディレクトリの後始末の失敗は握りつぶす
        let _ = fs::remove_dir_all(&staging);
    }
    installed
}

/// 読み取り位置から末尾までの SHA-256 を期待値(小文字 hex)と照合する
fn ensure_stream_sha256(reader: &mut impl io::Read, expected: &str, archive: &Path) -> Result<()> {
    let mut hasher = Sha256::new();
    io::copy(reader, &mut hasher)
        .with_context(|| format!("配布物を読めません: {}", archive.display()))?;
    let actual = format!("{:x}", hasher.finalize());
    ensure!(
        actual == expected,
        "配布物の SHA-256 が一致しません: {}(期待 {expected}、実際 {actual})",
        archive.display()
    );
    Ok(())
}

/// tar.gz から `DICTIONARY_FILES` の通常ファイルだけを `staging` 直下へ書き出す。
/// tar の項目の path は書き出し先の決定に使わない(書き出すファイル名は
/// `DICTIONARY_FILES` の定数だけ)ため、`..` や絶対 path を含む項目があっても
/// `staging` の外へは書かない
fn extract_dictionary_files(archive: impl io::Read, staging: &Path) -> Result<()> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    // 9 ファイルを含むディレクトリ(配布物の中の相対 path)。最初に見つけた
    // 辞書ファイルで決まり、以降は同じディレクトリのものだけを受け付ける
    let mut dict_parent: Option<PathBuf> = None;
    for entry in tar.entries().context("tar の項目を読めません")? {
        let mut entry = entry.context("tar の項目を読めません")?;
        let path = entry
            .path()
            .context("tar の項目の path を読めません")?
            .into_owned();
        let Some(name) = dictionary_file_name(&path) else {
            continue;
        };
        ensure!(
            entry.header().entry_type().is_file(),
            "辞書ファイルが通常ファイルではありません: {}",
            path.display()
        );
        let parent = path.parent().unwrap_or(Path::new("")).to_path_buf();
        match &dict_parent {
            Some(known) => ensure!(
                *known == parent,
                "辞書ファイルが複数のディレクトリにあります: {} と {}",
                known.display(),
                parent.display()
            ),
            None => dict_parent = Some(parent),
        }
        let out_path = staging.join(name);
        // create_new で重複した項目による上書きを拒む
        let mut out = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out_path)
        {
            Ok(out) => out,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                bail!("辞書ファイルが重複しています: {}", path.display())
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("辞書ファイルを作れません: {}", out_path.display()));
            }
        };
        io::copy(&mut entry, &mut out)
            .with_context(|| format!("辞書ファイルを書き出せません: {}", path.display()))?;
    }
    Ok(())
}

/// tar の項目の path が辞書ファイルを指すなら、`DICTIONARY_FILES` の名前を返す。
/// 先頭の `./` は無視する。`..`・絶対 path・3 階層以上の項目は辞書ファイルと
/// みなさない(`<ディレクトリ>/<ファイル>` か `<ファイル>` だけを受け付ける)
fn dictionary_file_name(path: &Path) -> Option<&'static str> {
    let mut normals = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normals.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if normals.len() > 2 {
        return None;
    }
    let file_name = normals.last()?.to_str()?;
    DICTIONARY_FILES
        .iter()
        .copied()
        .find(|name| *name == file_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// テストごとの作業ディレクトリ(プロセス ID とテスト名で一意)
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oboro-ja-dict-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 9 ファイルを持つ合成の辞書ディレクトリを作る(中身は名前由来のダミー)
    fn fake_dictionary(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        for name in DICTIONARY_FILES {
            fs::write(dir.join(name), format!("content of {name}")).unwrap();
        }
    }

    /// tar に通常ファイルの項目を 1 件足す
    fn append_file(builder: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        // set_path は `..` を拒むため、名前欄へ直接書いて悪意のある path も作れるようにする
        let name = &mut header.as_old_mut().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        builder.append(&header, data).unwrap();
    }

    /// 項目列から tar.gz を作り、(配布物の path、SHA-256 の hex)を返す
    fn write_archive(dir: &Path, entries: &[(&str, &[u8])]) -> (PathBuf, String) {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data) in entries {
            append_file(&mut builder, path, data);
        }
        gzip_to_file(dir, builder.into_inner().unwrap())
    }

    fn gzip_to_file(dir: &Path, tar_bytes: Vec<u8>) -> (PathBuf, String) {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        let bytes = encoder.finish().unwrap();
        let sha = format!("{:x}", Sha256::digest(&bytes));
        let archive = dir.join("dict.tar.gz");
        fs::write(&archive, bytes).unwrap();
        (archive, sha)
    }

    /// `package-dict.sh` と同じ形(`lindera-sudachidict/<9 ファイル>`)の項目列
    fn packaged_entries() -> Vec<(String, Vec<u8>)> {
        DICTIONARY_FILES
            .iter()
            .map(|name| {
                (
                    format!("lindera-sudachidict/{name}"),
                    format!("content of {name}").into_bytes(),
                )
            })
            .collect()
    }

    fn as_refs(entries: &[(String, Vec<u8>)]) -> Vec<(&str, &[u8])> {
        entries
            .iter()
            .map(|(path, data)| (path.as_str(), data.as_slice()))
            .collect()
    }

    #[test]
    fn verify_dir_reports_missing_files() {
        let dir = scratch("missing");
        fake_dictionary(&dir);
        fs::remove_file(dir.join("matrix.mtx")).unwrap();
        fs::remove_file(dir.join("unk.bin")).unwrap();
        let err = verify_dir(&dir).unwrap_err().to_string();
        assert!(err.contains("matrix.mtx, unk.bin"), "{err}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_dir_fingerprint_follows_content() {
        let dir = scratch("fingerprint");
        fake_dictionary(&dir);
        let first = verify_dir(&dir).unwrap();
        assert_eq!(first.dir, dir);
        assert_eq!(first.manifest_sha256, manifest_sha256(&dir).unwrap());
        // 同じ内容なら同じ fingerprint、内容が変われば変わる
        assert_eq!(verify_dir(&dir).unwrap(), first);
        fs::write(dir.join("unk.bin"), "changed").unwrap();
        assert_ne!(
            verify_dir(&dir).unwrap().manifest_sha256,
            first.manifest_sha256
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_sha256_accepts_bare_hex_and_shasum_output() {
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(parse_sha256(hex).unwrap(), hex);
        assert_eq!(
            parse_sha256(&format!("{hex}  jaio-dict.tar.gz\n")).unwrap(),
            hex
        );
        // 大文字は小文字へ揃える
        assert_eq!(parse_sha256(&hex.to_ascii_uppercase()).unwrap(), hex);
        let short = &hex[..63];
        let non_hex = format!("{short}g");
        for bad in ["", "abc", short, non_hex.as_str()] {
            assert!(parse_sha256(bad).is_err(), "{bad:?} は不正のはず");
        }
    }

    #[test]
    fn install_archive_places_verified_dictionary() {
        let dir = scratch("install");
        let entries = packaged_entries();
        let (archive, sha) = write_archive(&dir, &as_refs(&entries));
        let dest = dir.join("cache").join("sudachidict");
        // `.sha256` の内容(shasum 形式)をそのまま渡せる
        let installed = install_archive(&archive, &format!("{sha}  dict.tar.gz\n"), &dest).unwrap();
        assert_eq!(installed.dir, dest);
        assert_eq!(installed, verify_dir(&dest).unwrap());
        assert_eq!(
            fs::read_to_string(dest.join("dict.trie")).unwrap(),
            "content of dict.trie"
        );
        // 作業ディレクトリは残らない
        let leftovers: Vec<_> = fs::read_dir(dir.join("cache"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("sudachidict")]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_archive_rejects_checksum_mismatch() {
        let dir = scratch("mismatch");
        let entries = packaged_entries();
        let (archive, _) = write_archive(&dir, &as_refs(&entries));
        let dest = dir.join("dest");
        let wrong = "0".repeat(64);
        let err = install_archive(&archive, &wrong, &dest)
            .unwrap_err()
            .to_string();
        assert!(err.contains("SHA-256 が一致しません"), "{err}");
        assert!(!dest.exists(), "照合に失敗した配布物は置かない");
        assert!(verify_archive_sha256(&archive, &wrong).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_archive_rejects_incomplete_archive() {
        let dir = scratch("incomplete");
        let mut entries = packaged_entries();
        entries.retain(|(path, _)| !path.ends_with("dict.words"));
        let (archive, sha) = write_archive(&dir, &as_refs(&entries));
        let dest = dir.join("dest");
        let err = format!("{:#}", install_archive(&archive, &sha, &dest).unwrap_err());
        assert!(err.contains("dict.words"), "{err}");
        assert!(!dest.exists());
        // 失敗時は作業ディレクトリも残さない
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "配布物だけが残る");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_archive_writes_only_dictionary_files() {
        let dir = scratch("extra");
        let mut entries = packaged_entries();
        // macOS の tar が足す AppleDouble、辞書以外のファイル、`..` で外を指す項目、
        // 深い階層の同名ファイルは書き出さない
        entries.push((
            "lindera-sudachidict/._dict.trie".into(),
            b"appledouble".to_vec(),
        ));
        entries.push(("lindera-sudachidict/README.txt".into(), b"readme".to_vec()));
        entries.push(("../escaped.txt".into(), b"escape".to_vec()));
        entries.push(("../unk.bin".into(), b"escape".to_vec()));
        entries.push(("a/b/dict.trie".into(), b"deep".to_vec()));
        let (archive, sha) = write_archive(&dir, &as_refs(&entries));
        let dest = dir.join("out").join("dict");
        install_archive(&archive, &sha, &dest).unwrap();
        let mut names: Vec<String> = fs::read_dir(&dest)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, DICTIONARY_FILES);
        assert!(!dir.join("escaped.txt").exists());
        assert!(!dir.join("out").join("escaped.txt").exists());
        assert!(!dir.join("out").join("unk.bin").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_archive_rejects_ambiguous_layouts() {
        let dir = scratch("ambiguous");
        // 辞書ファイルが 2 つのディレクトリに分かれている
        let mut split = packaged_entries();
        split[0].0 = format!("other/{}", DICTIONARY_FILES[0]);
        let (archive, sha) = write_archive(&dir, &as_refs(&split));
        let err = format!(
            "{:#}",
            install_archive(&archive, &sha, &dir.join("d1")).unwrap_err()
        );
        assert!(err.contains("複数のディレクトリ"), "{err}");

        // 同じ辞書ファイルが 2 回現れる
        let mut twice = packaged_entries();
        twice.push(twice[0].clone());
        let (archive, sha) = write_archive(&dir, &as_refs(&twice));
        let err = format!(
            "{:#}",
            install_archive(&archive, &sha, &dir.join("d2")).unwrap_err()
        );
        assert!(err.contains("重複"), "{err}");
        assert!(!dir.join("d1").exists() && !dir.join("d2").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_archive_rejects_non_regular_dictionary_entry() {
        let dir = scratch("symlink");
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data) in packaged_entries().iter().skip(1) {
            append_file(&mut builder, path, data);
        }
        // 辞書ファイル名の symlink は、リンク先を読ませる攻撃になり得るため拒む
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Symlink);
        header
            .set_path(format!("lindera-sudachidict/{}", DICTIONARY_FILES[0]))
            .unwrap();
        header.set_link_name("/etc/passwd").unwrap();
        header.set_cksum();
        builder.append(&header, io::empty()).unwrap();
        let (archive, sha) = gzip_to_file(&dir, builder.into_inner().unwrap());
        let dest = dir.join("dest");
        let err = format!("{:#}", install_archive(&archive, &sha, &dest).unwrap_err());
        assert!(err.contains("通常ファイルではありません"), "{err}");
        assert!(!dest.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_archive_does_not_overwrite_existing_destination() {
        let dir = scratch("exists");
        let entries = packaged_entries();
        let (archive, sha) = write_archive(&dir, &as_refs(&entries));
        let dest = dir.join("dest");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("keep.txt"), "keep").unwrap();
        let err = install_archive(&archive, &sha, &dest)
            .unwrap_err()
            .to_string();
        assert!(err.contains("既に存在します"), "{err}");
        assert_eq!(fs::read_to_string(dest.join("keep.txt")).unwrap(), "keep");
        fs::remove_dir_all(&dir).ok();
    }
}

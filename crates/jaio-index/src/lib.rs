//! jaio-index: リポジトリ横断解析の索引アルゴリズム(設計書 §6.3)。
//! SimHash による重複候補列挙と n-gram Jaccard による検証を提供する。
//! 依存は最小に保ち、形態素解析や設定は扱わない。

use std::collections::HashSet;

/// 比較用に文字種を限定して正規化する(空白・句読点を除外し、ASCII は小文字化)。
pub fn normalize_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars().filter(|c| c.is_alphanumeric()) {
        out.extend(ch.to_lowercase());
    }
    out
}

/// 正規化済みテキストの文字 bigram 集合。
pub fn bigrams(text: &str) -> HashSet<[char; 2]> {
    let chars: Vec<char> = text.chars().collect();
    chars.windows(2).map(|w| [w[0], w[1]]).collect()
}

/// 文字 bigram の Jaccard 係数(0.0 - 1.0)。
pub fn bigram_jaccard(a: &str, b: &str) -> f64 {
    let (intersection, union) = bigram_jaccard_parts(a, b);
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

/// 文字 bigram の積集合・和集合の要素数を (積集合, 和集合) で返す。
/// 比率の分子・分母をレポートへ出すために使う(#34)。
pub fn bigram_jaccard_parts(a: &str, b: &str) -> (usize, usize) {
    let (a, b) = (bigrams(a), bigrams(b));
    let intersection = a.intersection(&b).count();
    (intersection, a.union(&b).count())
}

/// SimHash のハッシュ関数(FNV-1a 64bit)。特徴量の安定な分散のため std の
/// 既定ハッシュ(SipHash は乱数シード)を使わない。
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// bigram 1 個を FNV-1a でハッシュする(simhash と事前計算で共通利用)。
fn gram_hash(gram: &[char; 2]) -> u64 {
    let mut bytes = Vec::with_capacity(8);
    let mut buf = [0u8; 4];
    for ch in gram {
        bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
    fnv1a(&bytes)
}

/// 正規化済みテキストの SimHash フィンガープリント(設計書 §5.4)。
/// 文字 bigram を特徴量とし、ビットごとの重み付き多数決で 64bit に圧縮する。
pub fn simhash(text: &str) -> u64 {
    let mut bits = [0i64; 64];
    for gram in bigrams(text) {
        let hashed = gram_hash(&gram);
        for (bit, weight) in bits.iter_mut().enumerate() {
            if hashed & (1u64 << bit) != 0 {
                *weight += 1;
            } else {
                *weight -= 1;
            }
        }
    }
    let mut fingerprint = 0u64;
    for (bit, weight) in bits.iter().enumerate() {
        if *weight > 0 {
            fingerprint |= 1u64 << bit;
        }
    }
    fingerprint
}

/// bigram を u64 ハッシュの昇順ソート済み列として返す。
/// 全ペア比較の繰り返しに備えた事前計算用(`jaccard_sorted` と組み合わせる)。
/// ハッシュ化なので理論上は衝突で別 bigram を一致と数え得る近似だが、
/// FNV-1a 64bit では実用上無視できる
pub fn bigram_hashes(text: &str) -> Vec<u64> {
    let mut hashes: Vec<u64> = bigrams(text).iter().map(gram_hash).collect();
    hashes.sort_unstable();
    hashes
}

/// `containment_sorted` と同じマージ走査の途中結果を (積集合, needle の要素数)
/// で返す。比率の分子・分母をレポートへ出すために使う(#34)。
pub fn containment_parts_sorted(needle: &[u64], haystack: &[u64]) -> (usize, usize) {
    let mut i = 0usize;
    let mut j = 0usize;
    let mut intersection = 0usize;
    while i < needle.len() && j < haystack.len() {
        match needle[i].cmp(&haystack[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                intersection += 1;
                i += 1;
                j += 1;
            }
        }
    }
    (intersection, needle.len())
}

/// ソート済み bigram ハッシュ列同士の Jaccard 係数(0.0 - 1.0)。
/// マージによる積集合計算のため、ペアを大量に比較する場面で
/// `bigram_jaccard`(集合を都度構築)より高速。
pub fn jaccard_sorted(a: &[u64], b: &[u64]) -> f64 {
    let (intersection, union) = jaccard_parts_sorted(a, b);
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

/// `jaccard_sorted` と同じマージ走査の途中結果を (積集合, 和集合) で返す。
/// 比率の分子・分母をレポートへ出すために使う(#34)。
pub fn jaccard_parts_sorted(a: &[u64], b: &[u64]) -> (usize, usize) {
    let mut i = 0usize;
    let mut j = 0usize;
    let mut intersection = 0usize;
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                intersection += 1;
                i += 1;
                j += 1;
            }
        }
    }
    (intersection, a.len() + b.len() - intersection)
}

/// 類似度ヒストグラムの bin 幅(0.00-1.00 を 0.05 刻みで分割します)。
pub const SIMILARITY_BIN_WIDTH: f64 = 0.05;

/// 類似度ヒストグラムの bin 総数(0.00-1.00 を 0.05 刻み)。
pub const SIMILARITY_BIN_COUNT: usize = 20;

/// 類似度が帰属する bin の番号(0 起点)を返します。
/// 境界値はその値を下限とする bin に含めます(0.65 は 0.65-0.70 の bin)。
/// 1.0 は最終 bin に、範囲外の値は端の bin に丸めます。
pub fn similarity_bin_index(similarity: f64) -> usize {
    // f64 の除算丸めで境界値が 1 つ下の bin に落ちることがあるため、
    // 微小値を足してから切り下げる(例: 0.7 / 0.05 は 13.99... に丸まる)
    let scaled = similarity / SIMILARITY_BIN_WIDTH + 1e-9;
    scaled.floor().clamp(0.0, (SIMILARITY_BIN_COUNT - 1) as f64) as usize
}

/// 類似度列を 0.05 刻みの bin に集計し、(bin 下限, 件数) を下限の昇順で返します。
/// 閾値チューニングのための分布ダンプ(`jaio stats --similarity`)で使います。
pub fn similarity_histogram(similarities: &[f64]) -> Vec<(f64, usize)> {
    let mut counts = vec![0usize; SIMILARITY_BIN_COUNT];
    for &similarity in similarities {
        counts[similarity_bin_index(similarity)] += 1;
    }
    counts
        .into_iter()
        .enumerate()
        .map(|(idx, count)| (idx as f64 * SIMILARITY_BIN_WIDTH, count))
        .collect()
}

/// 類似度ペア列を類似度の降順(同点はパス名の昇順)に並べ替え、上位 `limit` 件を
/// 返します。順序は入力順に依存せず決定的です。
/// 類似度ペアの上位 `limit` 件(類似度降順・同点はインデックス順で決定的)。
/// ペア要素は文字列ではなくインデックスで持ち、呼び出し側で表示時に
/// 解決する(#44: パス文字列のペアごとの clone を避ける)
pub fn top_similar_pairs(
    mut pairs: Vec<(f64, usize, usize)>,
    limit: usize,
) -> Vec<(f64, usize, usize)> {
    pairs.sort_by(|a, b| {
        // 類似度は f64 のため NaN だけ順序を切る(実運用では発生しない)
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (a.1, a.2).cmp(&(b.1, b.2)))
    });
    pairs.truncate(limit);
    pairs
}

/// 2 つのフィンガープリントのハミング距離。
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 重複候補の列挙に使うハミング距離のしきい値。
pub const SIMHASH_MAX_DISTANCE: u32 = 3;

/// 全ペア比較を行う文書数の上限。
/// SimHash は日本語の短い文書では 1 語差でも距離が 6〜10 程度に広がる
/// (特徴量が少なく分離度が低い)ため、近重複の検出漏れを避けるには全ペアの
/// n-gram 照合が本質的に必要になる。事前計算したソート済み bigram ハッシュ列の
/// マージ比較により、この規模まで全ペア精査が数秒で終わる(実測)。
/// これを超える規模では距離 3 以下のみ保証のバンディングに切替える
/// (1 文差の近重複は検出漏れの可能性あり。設計書 §5.4、ISSUE.md 参照)。
pub const ALL_PAIRS_MAX_DOCS: usize = 2048;

/// バンディングの 1 バケットから列挙するメンバー数の上限(#102)。
/// 同一署名の大量集積(異常系コーパス)でのペア数爆発を防ぐ
pub(crate) const MAX_BUCKET_MEMBERS: usize = 512;

/// 候補列挙のバンド構成(16bit x 4 バンド)。
/// ハミング距離が `SIMHASH_MAX_DISTANCE`(3)以下の 2 署名は、鳩の巣原理により
/// 少なくとも 1 バンドで同じ値を共有する(異なるビットは高々 3 バンドにしか
/// 影響しないため)。
const BAND_WIDTH: u32 = 16;
const BAND_COUNT: u32 = 4;

/// 候補列挙で実際に使った方式の記録(§7.3 の `context.candidate_search`、#36)。
/// 近似検索での未検出は「全ペアを調べて類似なし」と同義ではないため、
/// 使った方式を黙って切り替えず出力に残す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairSearchMethod {
    /// 全ペアを n-gram 照合の候補にした(文書数が上限以下、または強制指定)
    AllPairs,
    /// SimHash バンディングによる近似候補列挙(距離 3 以下のみ保証)
    SimHashBanding,
}

/// 重複候補ペアと列挙方式(インデックス昇順、重複なし)。
/// `force_all_pairs` が真なら文書数に関わらず全ペアを候補にする
/// (近似検索との参照比較用、`detection.all_pairs`、#36)。
pub fn candidate_pairs(
    signatures: &[u64],
    force_all_pairs: bool,
) -> (PairSearchMethod, Vec<(usize, usize)>) {
    let n = signatures.len();
    if force_all_pairs || n <= ALL_PAIRS_MAX_DOCS {
        // n = 0 のアンダーフローを避けるため飽和演算にする
        let mut pairs = Vec::with_capacity(n.saturating_sub(1) * n / 2);
        for i in 0..n {
            for j in (i + 1)..n {
                pairs.push((i, j));
            }
        }
        return (PairSearchMethod::AllPairs, pairs);
    }
    use std::collections::{HashMap, HashSet};
    let mut buckets: HashMap<(u32, u16), Vec<usize>> = HashMap::new();
    for (i, &sig) in signatures.iter().enumerate() {
        for band in 0..BAND_COUNT {
            let value = (sig >> (band * BAND_WIDTH)) as u16;
            buckets.entry((band, value)).or_default().push(i);
        }
    }
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let mut pairs = Vec::new();
    for members in buckets.into_values() {
        // 1 バケットからの列挙メンバー数の上限(#102)。同一署名が大量に集まる
        // バケット(機械生成コーパス等の異常系)で members² のペア列挙が
        // 爆発するのを防ぐ。上限超過分は候補ペアから落ちる(= 詳細採点
        // されない)ため、通常のコーパスでは発動しない
        let members = &members[..members.len().min(MAX_BUCKET_MEMBERS)];
        for x in 0..members.len() {
            for y in (x + 1)..members.len() {
                let pair = if members[x] < members[y] {
                    (members[x], members[y])
                } else {
                    (members[y], members[x])
                };
                if seen.insert(pair) {
                    pairs.push(pair);
                }
            }
        }
    }
    pairs.sort_unstable();
    (PairSearchMethod::SimHashBanding, pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY_A: &str = "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれます。";

    #[test]
    fn normalizes_punctuation_and_case() {
        assert_eq!(normalize_text(" これ、は Test です。"), "これはtestです");
    }

    #[test]
    fn jaccard_of_identical_texts_is_one() {
        let a = normalize_text(BODY_A);
        assert!((bigram_jaccard(&a, &a) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_of_different_texts_is_low() {
        let a = normalize_text(BODY_A);
        let b = normalize_text("コマンドの終了コードと出力形式を定めます。");
        assert!(bigram_jaccard(&a, &b) < 0.3);
    }

    #[test]
    fn simhash_is_deterministic_for_identical_texts() {
        let a = normalize_text(BODY_A);
        assert_eq!(hamming_distance(simhash(&a), simhash(&a)), 0);
    }

    #[test]
    fn jaccard_of_near_identical_texts_is_high() {
        // 1 語の活用差は Jaccard で高類似と判定される。SimHash の近さは
        // 短いテキストでは保証されないため、既定規模(ALL_PAIRS_MAX_DOCS 以下)は
        // 全ペアを候補にして検出漏れを防ぐ設計(設計書 §5.4)
        let a = normalize_text(BODY_A);
        let b = normalize_text(
            "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込みます。",
        );
        assert!(bigram_jaccard(&a, &b) > 0.8);
    }

    #[test]
    fn simhash_of_different_texts_is_far() {
        let a = normalize_text(BODY_A);
        let b = normalize_text("rust の所有権システムはメモリ安全性をコンパイル時に保証します。");
        assert!(hamming_distance(simhash(&a), simhash(&b)) > SIMHASH_MAX_DISTANCE);
    }

    #[test]
    fn candidate_pairs_find_similar_texts() {
        // 1 語差の近重複は SimHash 距離が 6〜10 程度に広がるためバンディングだけでは
        // 漏れるが、既定規模(ALL_PAIRS_MAX_DOCS 以下)は全ペア候補のため検出される
        let a = normalize_text(BODY_A);
        let b = normalize_text(
            "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込みます。",
        );
        let signatures = [simhash(&a), simhash(&b)];
        let (method, pairs) = candidate_pairs(&signatures, false);
        assert_eq!(method, PairSearchMethod::AllPairs);
        assert!(pairs.contains(&(0, 1)));
    }

    #[test]
    fn banding_beyond_all_pairs_threshold_keeps_close_signatures() {
        // しきい値(ALL_PAIRS_MAX_DOCS)を超える規模ではバンディングに切替わる。
        // ハミング距離 3 以下のペアは鳩の巣原理により必ず候補になる(擬陰性なしの保証)
        let base: u64 = 0x1234_5678_9abc_def0;
        let mut signatures: Vec<u64> = Vec::new();
        for i in 1..(ALL_PAIRS_MAX_DOCS + 50) {
            signatures.push(
                base.rotate_left((i % 64) as u32) ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
            );
        }
        signatures.insert(0, base);
        signatures.push(base ^ 0b1011); // base とハミング距離 3
        let (method, pairs) = candidate_pairs(&signatures, false);
        assert_eq!(
            method,
            PairSearchMethod::SimHashBanding,
            "上限超過ではバンディングに切替わるはず"
        );
        // `detection.all_pairs`(force)なら規模に関わらず全ペアを候補にする(#36)
        let (forced_method, forced_pairs) = candidate_pairs(&signatures, true);
        assert_eq!(forced_method, PairSearchMethod::AllPairs);
        assert_eq!(
            forced_pairs.len(),
            signatures.len() * (signatures.len() - 1) / 2
        );
        assert!(
            pairs.contains(&(0, signatures.len() - 1)),
            "距離 3 のペアが候補から漏れた"
        );
        assert!(
            !pairs.contains(&(0, 1)),
            "遠いペアは通常候補に入らない(偶然のバンド一致の回帰監視)"
        );
    }

    #[test]
    fn banding_recall_is_measured_against_all_pairs_reference() {
        // #35: 同じ近重複ペアを保持した 2049 文書(全ペア上限超過)で、
        // 全ペア参照結果に対するバンディング候補の取りこぼしを測定する。
        // recall=100% を要求するのではなく、見落としを数値として把握できる
        // ことが条件。この fixture では植えたペアの捕捉を固定し、
        // 未検出を隠して合格にはしない
        // 16 倍した文書番号の hex が 1 桁ずれるため (i, 16i) 系の疑似類似ペアが
        // 多数でき、偶然でなく現実的な recall 計測になる
        let mut texts: Vec<String> = (0..(ALL_PAIRS_MAX_DOCS + 1) as u64)
            .map(|i| {
                normalize_text(&format!(
                    "文書{i:04} 記録{:x} {:x} {:x}",
                    i.wrapping_mul(0x9e37_79b9_7f4a_7c15),
                    i.wrapping_mul(0x85eb_ca6b_13c2_9eb9),
                    i.wrapping_mul(0xc2b2_ae35_d27e_4e4f),
                ))
            })
            .collect();
        // 植えた近重複ペア(先頭 2 件を同じ本文にする)。
        // シグネチャ距離 0 は必ずバンドを共有し、取りこぼし不能な正例
        texts[0] = normalize_text(BODY_A);
        texts[1] = normalize_text(BODY_A);

        let signatures: Vec<u64> = texts.iter().map(|t| simhash(t)).collect();
        let hashes: Vec<Vec<u64>> = texts.iter().map(|t| bigram_hashes(t)).collect();
        let cutoff = 0.5;

        // 参照結果: 全ペアを強制列挙して同じ前処理・同じ閾値で採点する
        let (_, exact_pairs) = candidate_pairs(&signatures, true);
        let exact_above: std::collections::HashSet<(usize, usize)> = exact_pairs
            .into_iter()
            .filter(|&(i, j)| jaccard_sorted(&hashes[i], &hashes[j]) >= cutoff)
            .collect();
        assert!(
            exact_above.contains(&(0, 1)),
            "植えたペアが参照結果にあるはず"
        );

        let (method, approx_pairs) = candidate_pairs(&signatures, false);
        assert_eq!(method, PairSearchMethod::SimHashBanding);
        let approx_set: std::collections::HashSet<(usize, usize)> =
            approx_pairs.into_iter().collect();
        let missed: Vec<(usize, usize)> = exact_above.difference(&approx_set).copied().collect();
        let recall = if exact_above.is_empty() {
            f64::NAN
        } else {
            (exact_above.len() - missed.len()) as f64 / exact_above.len() as f64
        };
        // 取りこぼしは隠さず計測値として残す(閾値超過数・未捕捉数・再現率)。
        // 近似検索の見落としは機構上あり得るため全件捕捉は要求しないが、
        // 植えた近重複ペア(距離 0)は必ず捕捉されなければならない
        eprintln!(
            "banding recall: {recall:.3} ({} / {} pairs, missed: {missed:?})",
            exact_above.len() - missed.len(),
            exact_above.len(),
        );
        assert!(
            approx_set.contains(&(0, 1)),
            "植えた近重複ペアが近似候補から漏れた"
        );
        assert!(exact_above.len() > 1, "fixture が自明でないことを確認");
    }

    #[test]
    fn all_pairs_boundary_is_exact() {
        // ちょうど上限(2048)では全ペア、上限+1(2049)でバンディングに
        // 切替わる(#35)。境界の off-by-one を固定する
        let at: Vec<u64> = (0..ALL_PAIRS_MAX_DOCS as u64)
            .map(|i| i.wrapping_mul(0x9e37_79b9_7f4a_7c15))
            .collect();
        let (method, pairs) = candidate_pairs(&at, false);
        assert_eq!(method, PairSearchMethod::AllPairs);
        assert_eq!(
            pairs.len(),
            ALL_PAIRS_MAX_DOCS * (ALL_PAIRS_MAX_DOCS - 1) / 2
        );

        let over: Vec<u64> = (0..(ALL_PAIRS_MAX_DOCS + 1) as u64)
            .map(|i| i.wrapping_mul(0x9e37_79b9_7f4a_7c15))
            .collect();
        let (method, _) = candidate_pairs(&over, false);
        assert_eq!(method, PairSearchMethod::SimHashBanding);
    }

    #[test]
    fn jaccard_sorted_agrees_with_set_based_jaccard() {
        let a = normalize_text(BODY_A);
        let b = normalize_text(
            "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込みます。",
        );
        let by_sorted = jaccard_sorted(&bigram_hashes(&a), &bigram_hashes(&b));
        let by_set = bigram_jaccard(&a, &b);
        assert!(
            (by_sorted - by_set).abs() < 1e-9,
            "ソート済み比較と集合ベースが一致するはず: {by_sorted} vs {by_set}"
        );
        assert!(by_sorted > 0.8);
    }

    #[test]
    fn jaccard_sorted_handles_empty() {
        assert_eq!(jaccard_sorted(&[], &[1, 2]), 0.0);
        assert_eq!(jaccard_sorted(&bigram_hashes("同じ本文"), &[]), 0.0);
    }

    #[test]
    fn similarity_histogram_and_top_pairs_are_deterministic() {
        // bin 境界値の帰属: 境界はその値を下限とする bin に入る(0.65 は 0.65-0.70)
        assert_eq!(similarity_bin_index(0.0), 0);
        assert_eq!(similarity_bin_index(0.049), 0);
        assert_eq!(similarity_bin_index(0.05), 1);
        assert_eq!(similarity_bin_index(0.6), 12);
        assert_eq!(similarity_bin_index(0.65), 13);
        assert_eq!(similarity_bin_index(1.0), SIMILARITY_BIN_COUNT - 1);
        assert_eq!(similarity_bin_index(1.5), SIMILARITY_BIN_COUNT - 1);
        assert_eq!(similarity_bin_index(-0.1), 0);

        // 0.05 刻みで全 bin が下限昇順に並び、件数の総和が一致する
        let histogram = similarity_histogram(&[0.0, 0.12, 0.64, 0.65, 0.65, 1.0]);
        assert_eq!(histogram.len(), SIMILARITY_BIN_COUNT);
        assert_eq!(histogram[0].1, 1);
        assert_eq!(histogram[2].1, 1); // 0.12 は 0.10-0.15
        assert_eq!(histogram[12].1, 1); // 0.64 は 0.60-0.65
        assert_eq!(histogram[13].1, 2); // 0.65 は 0.65-0.70
        assert_eq!(histogram[19].1, 1); // 1.0 は最終 bin
        assert_eq!(histogram.iter().map(|&(_, c)| c).sum::<usize>(), 6);
        for (idx, (lower, _)) in histogram.iter().enumerate() {
            assert!((lower - idx as f64 * SIMILARITY_BIN_WIDTH).abs() < 1e-9);
        }

        // 同点はインデックス順で決定的に並ぶ(類似度は降順、#44 のインデックス化)
        let pairs = vec![
            (0.9, 1usize, 3usize),
            (0.9, 0usize, 2usize),
            (0.95, 4usize, 5usize),
        ];
        let top = top_similar_pairs(pairs, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0], (0.95, 4, 5));
        assert_eq!(top[1], (0.9, 0, 2));
    }
}

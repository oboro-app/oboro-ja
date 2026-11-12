//! jaio-core: 診断モデル・設定を置く核心 crate(依存最小)。
//!
//! 設計の正本は `docs/design.md`。ルール trait は DocContext が jaio-parse の型を
//! 参照するため jaio-rules 側に置く(設計書 §6.1)。

pub mod comments;
pub mod config;

/// パターン系設定の照合用に絶対パスを基準ディレクトリ(jaio.toml 親 → cwd)
/// 基準で相対化する(#76)。config モジュールで定義し、ルール側からも使う
pub use config::{normalize_lexical, relativize_for_match, resolved_base};

use std::fmt;

/// 診断の重大度。rustc の warn/deny 文化に倣う(設計書 §5)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// error: deny 相当。exit code 1 の原因になる
    Error,
    /// warn: 既定の重大度
    Warn,
    /// info: 参考情報
    Info,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Severity::Error => "error",
            Severity::Warn => "warning",
            Severity::Info => "info",
        };
        f.write_str(s)
    }
}

/// ドキュメント内の位置(1 基点)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Span {
    /// 行番号(1 基点)
    pub line: usize,
    /// 桁番号(1 基点。テキストリーフ内相対の近似値)
    pub col: usize,
}

/// 計測値の状態(設計書 §7.3)。未計測を 0 で埋めないための区別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementStatus {
    /// 計測済み(value を出力する)
    Measured,
    /// この診断では対象外
    NotApplicable,
    /// 標本不足で判定に足りない
    InsufficientSample,
    /// 計測手段がなく値を出せない
    Unavailable,
}

/// 計測値 1 件(設計書 §7.3)。metric はルールカタログの定義名、method は
/// 算式・算法の識別子とバージョン。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Measurement {
    /// 指標 ID(例: "bigram_jaccard", "token_count", "occurrence_count")
    pub metric: &'static str,
    /// 算式・算法の識別子(例: "bigram-jaccard" / "simhash64+bigram-jaccard")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<&'static str>,
    /// 計測状態
    pub status: MeasurementStatus,
    /// 測定値(status = measured のときのみ出力)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// 単位("count" / "ratio" / "tokens" / "chars" 等)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<&'static str>,
    /// 比率の根拠となる分子
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numerator: Option<f64>,
    /// 比率の根拠となる分母・標本数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denominator: Option<f64>,
    /// 判定閾値
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// 比較方向(">=" / "<=" / "<" / ">")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<&'static str>,
    /// 測定の対象範囲(例: "document_pair" / "leaf" / "corpus" / "symbol:、")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// measured 以外のとき、計測不能・対象外の理由
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Measurement {
    /// 計測済みの値を作る。非有限値(NaN/Infinity)は契約違反のため拒否する
    /// (シリアライズ時の null 化に頼らない、#32)
    pub fn measured(metric: &'static str, value: f64) -> Self {
        assert!(
            value.is_finite(),
            "measured の値は有限である必要があります: {metric}={value}"
        );
        Self {
            metric,
            method: None,
            status: MeasurementStatus::Measured,
            value: Some(value),
            unit: None,
            numerator: None,
            denominator: None,
            threshold: None,
            comparison: None,
            scope: None,
            reason: None,
        }
    }

    /// 計測不能・対象外の値を作る(理由つき)。measured や空の理由は
    /// 契約違反のため拒否する(#32)
    pub fn unmeasured(metric: &'static str, status: MeasurementStatus, reason: &str) -> Self {
        assert_ne!(
            status,
            MeasurementStatus::Measured,
            "unmeasured に measured は使えません: {metric}"
        );
        assert!(
            !reason.is_empty(),
            "unmeasured には空でない理由が必要です: {metric}"
        );
        Self {
            metric,
            method: None,
            status,
            value: None,
            unit: None,
            numerator: None,
            denominator: None,
            threshold: None,
            comparison: None,
            scope: None,
            reason: Some(reason.to_string()),
        }
    }

    pub fn method(mut self, method: &'static str) -> Self {
        self.method = Some(method);
        self
    }

    pub fn unit(mut self, unit: &'static str) -> Self {
        self.unit = Some(unit);
        self
    }

    pub fn ratio(mut self, numerator: f64, denominator: f64) -> Self {
        self.numerator = Some(numerator);
        self.denominator = Some(denominator);
        self
    }

    pub fn threshold(mut self, threshold: f64, comparison: &'static str) -> Self {
        self.threshold = Some(threshold);
        self.comparison = Some(comparison);
        self
    }

    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// status と value/reason の整合を検査する(schema §7.3 の状態別制約と同じ、#32)。
    /// 公開フィールドの直接変更で作られた矛盾状態を出力前に検出するため、
    /// レポート生成時に全件呼ぶ。違反はバグとして panic する。
    pub fn validate(&self) {
        match self.status {
            MeasurementStatus::Measured => assert!(
                self.value.is_some_and(|v| v.is_finite()),
                "measured には有限の value が必要です: {}",
                self.metric
            ),
            _ => {
                assert!(
                    self.value.is_none(),
                    "measured 以外が value を持てません: {}",
                    self.metric
                );
                assert!(
                    self.reason.as_deref().is_some_and(|r| !r.is_empty()),
                    "measured 以外には空でない reason が必要です: {}",
                    self.metric
                );
            }
        }
    }
}

/// 根拠位置 1 件(設計書 §7.3)。比較両側の位置を同じ形で表す。
/// スニペットは行番号だけでは分からない該当箇所を、レポート表示と
/// LLM 委譲(Issue #19)の両方で直接伝えるために持つ。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Location {
    /// 対象ファイルの表示パス
    pub file: String,
    /// 位置(1 基点)
    pub span: Span,
    /// 範囲終端。正確に取れない場合は出力しない
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<Span>,
    /// 該当テキストの断片(先頭から数十字に切り詰め、改行は空白化)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// 役割: "primary" / "counterpart"(比較相手) / "occurrence"(出現箇所)
    pub role: &'static str,
    /// 近似位置なら true(形態素・平文化後の位置から正確なソース位置が
    /// 取れない場合に明示する)
    pub approximate: bool,
}

impl Location {
    /// 出現箇所(役割 occurrence)を作る
    pub fn occurrence(file: String, span: Span, snippet: String) -> Self {
        Self {
            file,
            span,
            end: None,
            snippet: Some(snippet),
            role: "occurrence",
            approximate: false,
        }
    }

    /// 比較相手の位置(役割 counterpart)を作る
    pub fn counterpart(file: String, span: Span) -> Self {
        Self {
            file,
            span,
            end: None,
            snippet: None,
            role: "counterpart",
            approximate: false,
        }
    }
}

/// 文頭から `max_chars` 字の断片を作る(マルチバイト境界安全)。
/// 連続する空白・改行は 1 つの空白に正規化し、レポートを 1 行に保つ。
pub fn snippet(text: &str, max_chars: usize) -> String {
    let normalized: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() <= max_chars {
        normalized
    } else {
        chars[..max_chars].iter().collect()
    }
}

/// 診断 1 件(schema v2、設計書 §7.3)。
/// 観察(observation)・計測値(measurements)・根拠位置(locations)・
/// 確認事項(review_questions)・限界(limitations)を分離して持つ。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Diagnostic {
    /// 対象ファイルの表示パス
    pub file: String,
    /// 位置
    pub span: Span,
    /// ルール ID(例: "consistency/style-deviation")
    pub rule_id: &'static str,
    /// 重大度(規約上のもの。測定値・確信度・レビュー優先度とは分離)
    pub severity: Severity,
    /// 観察(計測値から直接言えること)
    pub observation: String,
    /// AI・人間が文脈で判断する確認事項
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub review_questions: Vec<String>,
    /// 計測値列
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub measurements: Vec<Measurement>,
    /// 追加の根拠位置列(主位置は span)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<Location>,
    /// 解釈上の限界
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

impl Diagnostic {
    /// ルール ID のカテゴリ(`/` の前、設計書 §5)
    pub fn category(&self) -> &'static str {
        self.rule_id.split('/').next().unwrap_or(self.rule_id)
    }

    /// 最小の診断を作る(位置・ルール・重大度・観察のみ)
    pub fn new(file: String, span: Span, rule_id: &'static str, observation: String) -> Self {
        Self {
            file,
            span,
            rule_id,
            severity: Severity::Warn,
            observation,
            review_questions: Vec::new(),
            measurements: Vec::new(),
            locations: Vec::new(),
            limitations: Vec::new(),
        }
    }

    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// 確認事項を 1 件足す
    pub fn review_question(mut self, question: impl Into<String>) -> Self {
        self.review_questions.push(question.into());
        self
    }

    /// 計測値を 1 件足す
    pub fn measurement(mut self, m: Measurement) -> Self {
        self.measurements.push(m);
        self
    }

    /// 根拠位置を 1 件足す
    pub fn location(mut self, l: Location) -> Self {
        self.locations.push(l);
        self
    }

    /// 解釈上の限界を 1 件足す
    pub fn limitation(mut self, l: impl Into<String>) -> Self {
        self.limitations.push(l.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_display() {
        assert_eq!(Severity::Error.to_string(), "error");
        assert_eq!(Severity::Warn.to_string(), "warning");
        assert_eq!(Severity::Info.to_string(), "info");
    }

    #[test]
    fn measurement_builder_sets_fields() {
        let m = Measurement::measured("bigram_jaccard", 0.86)
            .method("simhash64+bigram-jaccard")
            .unit("ratio")
            .threshold(0.65, ">=")
            .scope("document_pair");
        assert_eq!(m.status, MeasurementStatus::Measured);
        assert_eq!(m.value, Some(0.86));
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["metric"], "bigram_jaccard");
        assert_eq!(json["status"], "measured");

        // 計測不能は value を出さず理由を持つ
        let u = Measurement::unmeasured(
            "token_delta",
            MeasurementStatus::Unavailable,
            "編集後の全体計測はできない",
        );
        let json = serde_json::to_value(&u).unwrap();
        assert_eq!(json["status"], "unavailable");
        assert!(json.get("value").is_none());
        assert_eq!(json["reason"], "編集後の全体計測はできない");
    }

    #[test]
    fn measured_rejects_non_finite() {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                std::panic::catch_unwind(|| Measurement::measured("x", v)).is_err(),
                "非有限値 {v} は拒否されるはず"
            );
        }
        // 有限のゼロは正当な測定値として受理する
        assert_eq!(
            Measurement::measured("x", 0.0).value,
            Some(0.0),
            "測定値 0 は「計測不能」と区別されるはず"
        );
    }

    #[test]
    fn unmeasured_rejects_measured_status_and_empty_reason() {
        assert!(
            std::panic::catch_unwind(|| {
                Measurement::unmeasured("x", MeasurementStatus::Measured, "理由")
            })
            .is_err()
        );
        assert!(
            std::panic::catch_unwind(|| {
                Measurement::unmeasured("x", MeasurementStatus::Unavailable, "")
            })
            .is_err()
        );
        // 3 つの非測定状態は理由つきで受理する
        for s in [
            MeasurementStatus::NotApplicable,
            MeasurementStatus::InsufficientSample,
            MeasurementStatus::Unavailable,
        ] {
            let m = Measurement::unmeasured("x", s, "理由あり");
            m.validate();
        }
    }

    #[test]
    fn validate_detects_mutated_inconsistency() {
        // 公開フィールドを直接書き換えた矛盾状態を検出する
        let mut m = Measurement::measured("x", 1.0);
        m.value = None;
        assert!(std::panic::catch_unwind(|| m.validate()).is_err());

        let mut u = Measurement::unmeasured("x", MeasurementStatus::Unavailable, "理由");
        u.value = Some(0.5);
        assert!(std::panic::catch_unwind(|| u.validate()).is_err());
    }
}

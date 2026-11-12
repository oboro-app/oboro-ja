# jaio 設計書

- ステータス: 設計確定(Phase 1 実装済み)
- 対象読者: 本ツールの実装者、および本設計のレビュー担当

## 1. 概要

### 1.1 目的

リポジトリ内の日本語文書を静的解析し、次の 2 つの観点から問題を検出・報告します。

1. **人間の可読性**: 表記揺れ、文体の逸脱、構造の乱れ
2. **AI のトークン効率**: 冗長表現、重複コンテンツによるコンテキストウィンドウの消費

「文書版 clippy」を目指します。診断の形式(重大度、rule ID、help メッセージ)と運用(warn/deny、設定ファイル)は Rust の linter 文化に倣います。

### 1.2 スコープ

対象:

- Markdown 文書(`.md` / `.mdx`)
- ソースコード内コメント(Rust, TypeScript, Python, Go 等)
- ディレクトリ構造と文書の関係(リンクグラフ、ディレクトリ役割)

対象外:

- 意味的冗長性の最終判断 — LLM/人間に委譲(§2)
- 文書の自動リライト・自動修正全般 — 読み取り専用方針(§7.4、#31)のため実装しない
- 英語文書の解析 — 日本語前提。n-gram 等の一部ロジックは言語非依存

### 1.3 確定した設計判断

| 論点 | 決定 | 理由 |
|---|---|---|
| セマンティック解析(埋め込みモデル) | 組み込まない | ビルド・バイナリサイズを軽量に保つ。意味判断は LLM に任せ、jaio は材料の提供に専念 |
| 形態素解析辞書 | Sudachi | 複数粒度(A: 短単位 / C: 長単位)の切り替えが可能、新語に強い |
| 第一出力 | CLI レポート | 手元実行での確認を最初に固める。JSON は Phase 2 で第一級出力に追加 |
| 自動修正(旧 `--fix`) | 実装しない(#33 で廃止) | 読み取り専用解析器として位置づけ直した(§7.4、#31)。置き換え候補は非拘束の材料として出力に残す |
| 求める文体 | 丁寧(ですます体)かつ無駄のない文章 | 本ツールが対象リポジトリに求める既定の文書規約。`style-deviation` は敬体を正統側として逸脱を報告 |

## 2. ポジショニング: 決定論的解析 × LLM 委譲

責務を次のように分離します。

| 主体的 | 責務 |
|---|---|
| jaio | 決定論的解析(構文・統計・グラフ)による診断と候補の構造化出力 |
| LLM / 人間 | 提供された材料からの意味判断と最終編集 |

- 重複検出は **SimHash + n-gram + TF-IDF + Jaccard 係数** の決定論的手法のみ
- 統合提案は「候補ペア + 類似度スコア + 根拠」までを出力し、実行はしません
- `--format json` を第一級の出力とし、LLM エージェントが `jaio check --format json` を実行 → 診断 JSON を読む → 判断・編集、というワークフローに直接乗れる設計にします

## 3. 全体アーキテクチャ

```
┌─────────┐   ┌─────────┐   ┌─────────┐   ┌──────────┐   ┌─────────┐
│ Collect │──▶│  Parse  │──▶│  Index  │──▶│ Analyze  │──▶│ Report  │
└─────────┘   └─────────┘   └─────────┘   └──────────┘   └─────────┘
 ファイル走査    文書AST構築     横断索引       ルール実行      出力
 コメント抽出   日本語AST構築   用語転置索引   Document /     terminal
 (ignore)     (comrak +     SimHash署名    Corpus の      json / md
                lindera)     リンクグラフ    2 種ルート      (sarif)
```

| 段階 | 内容 |
|---|---|
| Collect | `ignore` クレートで走査(.gitignore 準拠。`.git` の有無に依らず適用、#58。この際、グローバル gitignore(`~/.config/git/ignore` 等)も適用対象になる — ripgrep と同様の挙動)。Markdown とソースコードを列挙 |
| Parse | `rayon` でファイル単位に並列。Markdown → 文書構造 AST、テキストリーフ → 日本語 AST。ソースコードは `tree-sitter` でコメントノードのみ抽出し同じパイプラインへ |
| Index | リポジトリ横断の転置索引(原形ベース)、SimHash 署名、リンクグラフ、トークン統計 |
| Analyze | ルールエンジンが Document ルール(単一文書内)と Corpus ルール(横断)を実行 |
| Report | 診断の集約・フィルタ・出力 |

## 4. データモデル: 3 層 AST

### 4.1 層 1 — 文書構造 AST(Markdown)

`comrak` で MDAST を構築し、次のノードへ整理します。

```
Document
└── Section(見出しツリー + セクション役割分類: 概要/手順/注意/参照...)
    └── Block(段落 / リスト項目 / 表セル / コードブロック)
        └── テキストリーフ → 層 2 の日本語 AST を紐付け
```

コードブロックは本文解析の対象から除外します(内容の日本語解析は行わない)。インラインコードはテキストとして本文に含めます。言語タグは保持していません(§4.2 の実装の実態と一致)。

### 4.2 層 2 — 日本語 AST(文レベル)

形態素解析(`lindera` + Sudachi)を基礎に、次の構造で文レベルの解析を提供します。

```rust
/// 文レベルの解析結果(実装の実態)
pub struct Sentence {
    pub text: String,           // 文テキスト
}

/// 形態素の簡易表現(ルールで必要なフィールドのみ)
pub struct Morph {
    pub surface: String,        // 表層形
    pub lemma: String,          // 正規化表記(Sudachi の normalized_form)
    pub pos1: String,           // 品詞1(大分類)
    pub pos2: String,           // 品詞2(中分類)
    pub conjugation_form: String, // 活用形
    pub reading: String,        // 読み(カタカナ)
}

/// 文体の分類(文末形態素から判定)
pub enum SentenceStyle { Polite, Plain, Neutral }
```

係り受け解析器の Rust ネイティブ実装は乏しいため、文節を構造化せず
「文分割 → 形態素列 → ルール内で直接走査」の簡易構造としています
(`consistency/particle` は形態素列を直接走査して述語と助詞を関連付けます)。
当初構想にあった文節(Chunk)・係り受けヒューリスティック(`dep_to`)は
実装しておらず、必要になった時に導入します(#44 の整理)。形態素解析は
`Analyzer` の背後に隠し、将来の差し替え(ONNX による高精度係り受け等)に備えます。

lindera v6 の `Mode` は `Normal` / `Decompose(Penalty)` の 2 種のみで、Sudachi 本家の分割粒度 A/B/C は `Mode` としては提供されません(どの粒度のエントリが採択されたかは各形態素の `split_mode` フィールドで参照できます)。`jaio.toml` では `analysis.mode = "normal" | "decompose"` を指定し、既定は `normal` とします。粒度相当の使い分けが必要になったら `split_mode` 参照を評価します。

### 4.3 層 3 — リポジトリグラフ

- **文書 ↔ 文書**: Markdown 内リンク(相対パス解決)から被リンク集合を構築(`HashSet`。到達可能性解析が必要になったらグラフ基盤を導入します)。画像記法 `![alt](url)` は broken-link の実在検証には使いますが、**被リンク集計(orphan・統合候補)には数えません**(#74: alt テキストの参照はリンク意味ではないため)
- **文書 ↔ ソースコード**: `tree-sitter` で抽出したコメントテキストも層 2 まで解析し、文書との類似度を比較
- **ディレクトリ ↔ 文書**: README の有無からディレクトリ役割を推定し、README の用語と配下文書内容の整合を確認(ディレクトリ名との比較は実測で精度が出ず対象外)

## 5. 解析ルールカタログ

重大度は `error` / `warn` / `info`。設定で上書き可能です(§8)。

### 5.1 `efficiency` — トークン効率

| ID | 検出 | 方法 |
|---|---|---|
| `efficiency/redundant-expression` | 定型冗長表現(「〜することが可能です」→「〜できます」等) | パターン辞書(同梱 + ユーザー拡張 TOML) |
| `efficiency/filler` | フィラー句・空虚修飾(「〜と言っても過言ではない」等) | パターン辞書(`[patterns.redundant]` で置き換え先が空のもの = 削除候補) |
| `efficiency/token-density` | 情報密度の低い段落 | トークン数に対するコンテンツ語(名詞・動詞)比率の統計的外れ値。散文前提: 本文に占める段落が `detection.prose_min_share` 未満の文書(リスト主導)は対象外 |
| `efficiency/repeated-quote` | 同一文の重複出現(引用コピペ等) | 正規化済みの文(30 字以上)の完全一致集計。散文前提: リスト主導の文書は反復が構造的なため対象外 |
| `efficiency/long-sentence` | 長い文(分割候補) | 本文リーフ内の文について、非 ASCII 文字(全角記号・全角数字を含む「和文文字」の実装定義)の数が `detection.long_sentence_min_chars`(既定 100)以上の文を報告。ASCII(英字・URL・コード)は読み負荷が軽いため長さに数えない(#48: 英字列挙による誤検出の実測あり)。長文は形態素分解の誤読リスクを上げるため token-density(段落単位)と文単位で補完する。observation に文スニペットを含む(#59) |
| `efficiency/no-comma` | 読点を含まない長い文 | 非 ASCII 文字数が `detection.no_comma_min_chars`(既定 60)以上で、読点(`、`/`，`、非 ASCII 文字に後続する ASCII `,`・列挙中黒 `・`)を含まない文を報告。読点なし長文は修飾関係の曖昧性リスクの代理指標(#48)。中黒区切りのラベル列挙は読解の区切りとして機能するため、非 ASCII 文字に後続する中黒を pause に数える(#60)。observation に文スニペットを含む(#59) |

効率カテゴリの提案は目標文体(§1.3)を保存します。敬体は「です・ます」の分だけトークンを要するためトークン効率とはトレードオフになりますが、「〜します」を「〜する」へ変換する類の削減提案は行いません。

### 5.2 `consistency` — 揺れ

| ID | 検出 | 方法 |
|---|---|---|
| `consistency/notation-variant` | 同一語の表記ゆれ(ない/無い、データベース/データーベース) | (品詞, 読み) → 表記の索引。同一キーに複数表記が紐づくケースを検出し、加えて表記間の文字共通性(2 字以上)で同語疑似ペアも検出(実運用測定による拡張) |
| `consistency/terminology` | 用語揺れ(リポジトリ/レポジトリ 等の同義語ペア) | 人手管理のペア辞書(内蔵 + `jaio.toml [terms]` 拡張)で文中出現を集計。名詞は同音異義の誤検出が多いため原形・読みによる自動検出は行わない(実運用測定による) |
| `consistency/style-deviation` | ですます体を既定とした文体の逸脱(である体の混入) | 文末語尾の形態素分類。敬体が優勢(敬体数 >= 常体数)の文書では常体文を逸脱として warn、常体が優勢の文書には文書レベルの info を出す(既定文体は §1.3)。**文書単位の文体宣言が可能**(#55): frontmatter の `jaio-style: polite|plain` が最優先、次に `[style.overrides]` のパスパターン(gitignore 構文)、いずれも無ければ `[style] target`。素材・テンプレート・プロンプト系の「ですます体が正当な文書」を構造的に許容する |
| `consistency/particle` | てにをは: 同一述語に対する格助詞の揺れ | 述語(原形)→ 格助詞列の集計。報告ペアは `detection.particle_pairs` で opt-in(既定は空)。実測(#23)で決定論的には正当な使い分けと区別不能と判明。意味判断は LLM 委譲 |
| `consistency/punctuation` | 句読点の表記揺れ(読点 `、`/`，`/`,`、句点 `。`/`．`/`.` の混在) | 本文リーフ内の各表記の**出現数**を集計し、少数派を報告(#42: リーフ数だと 1 リーフ内の大量出現が多数派判定に反映されなかった)。報告位置は表記を含むリーフ単位。ASCII の `,`/`.` は数値・英語・URL と区別するため「直前が和文文字(ひらがな・カタカナ・漢字)、直後が和文文字・空白・行末・閉じ括弧」の文脈にあるものだけを和文の句読点と見なす |

文体判定の対象は段落(Paragraph)のみです。箇条書き(体言止めが慣例)と表セル・見出しは対象外とします。セクション単位の集計(見出し役割ごとの使い分け許容)は Phase 2 で評価します。

### 5.3 `structure` — Markdown 構造

| ID | 検出 | 方法 |
|---|---|---|
| `structure/heading-skip` | 見出しレベルの跳び(h2 → h4) | MDAST トラバーサル |
| `structure/empty-section` | 本文のない見出し | セクション内ブロック数 |
| `structure/over-nesting` | 過剰なリスト・引用のネスト | ネスト深さしきい値 |
| `structure/section-duplication` | 文書内の重複セクション | セクション本文類似度(Jaccard)で判定。見出し類似度は診断の表示に併記するのみ |

### 5.4 `relation` — リポジトリ横断の関係

| ID | 検出 | 方法 |
|---|---|---|
| `relation/cross-doc-duplication` | 文書間の重複記述 | SimHash で候補ペアを列挙 → 文書本文の Jaccard で検証 → 最も類似するセクション組を見出し行で報告(重複位置の特定)。各文書について類似度上位 3 件の相手文書まで warn で報告し、残りは「ほか N 文書と類似」の 1 件の info に集約する(生成文書のように重複が多いコーパスで診断が爆発するのを防ぐ)。ペアは `a < b` の向きだが報告は両側の文書それぞれに付ける — 常に `a` 側になる文書が診断ゼロになるのを防ぐため |
| `relation/doc-comment-drift` | コメントと文書の内容重複 | `jaio-collect`(tree-sitter)で抽出した日本語コメント本文と文書本文の bigram Jaccard 類似度。閾値以上を重複として報告する(乖離の判定は意味判断が必要なため LLM 委譲とし、決定論的には重複のみ検出する) |
| `relation/orphan` | 一切参照されない文書 | リンクグラフの孤立ノード。エントリポイント(`detection.orphan_entry_points`、既定は README.md)とコマンドラインで明示指定したファイルは対象外 |
| `relation/broken-link` | リンク切れ | 文書内の相対リンクの解決失敗。外部 URL(`http(s)`/`mailto`)・アンカーのみ・絶対パス(`/` 始まり)は対象外。**GitHub の PR/Issue テンプレート(`.github` 配下の文書)の相対リンクは repo ルート基準で解決する**(GitHub のレンダリング挙動に合わせる。.github は必ずルート直下のため `.github` の親を基準にする、#84/#86: テンプレート以外の .github 配下の文書は通常どおりファイル基準)。**画像 `![alt](url)` の相対 URL も実在チェックする**(#74: 画像のリンク切れは可読性の問題のため検証対象に含める。拡張子なし URL への `.md`/`.mdx` 補完は行わない)。URL デコードを行い、拡張子付きリンクはそのまま実在チェック、拡張子なしリンクは `.md`/`.mdx` を補完して判定する(画像は補完しない)。ディレクトリへのリンクは実在すれば有効 |
| `relation/move-integrity` | 移動宣言のある文書の行単位保全(#56) | `[verification.moves]` で宣言した移動元→移動先ペアについて、`verify-move` と同じ正規化(見出しマーカー除去・リンク簡約・trim)で実質行の**多重集合照合**(同一行は出現回数で判定、#80)を行い、未保全行を warn で報告。移動元が解析対象から消えていれば宣言の残骸として info、移動先が無ければ warn。宣言が空なら対象外 |
| `relation/dir-mismatch` | ディレクトリ役割と文書内容の不一致 | README の用語整合(README に現れる役割語彙と配下文書内容の一致)。ディレクトリ名との比較は実測で精度が出ず対象外 |

### 5.5 `merge` — 統合提案(判断は委譲)

| ID | 出力 | 方法 |
|---|---|---|
| `merge/consolidation-plan` | 重複グループ + 正候補スコア + 正候補との直接類似度 | `relation` の結果からプラン生成 |

正候補スコアの要素(重みは設定可能):

- 被リンク数(多く参照されている方を残す)
- 更新の新しさ(既定の重みは 0 で未使用。`merge.weights.recency > 0` で有効化したときだけ mtime を入力に使う)
- ファイル役割(README > 個別ページ、等の役割スコア)

`canonical_score` はこれらの重み付き和であり、0〜1 の比率ではありません。正候補との直接ペアが検出済みリストに無い場合は bigram Jaccard を直接計算した実測値を示し、グループ内の別辺の類似度は転用しません。文書全体のトークン削減量は本文の語彙類似度から推計しません(対象範囲が一致しないため `estimated_token_reduction` は `unavailable` とします)。

## 6. コンポーネント設計

### 6.1 ルールエンジン

ルールは対象スコープごとに 2 種類の trait に分けます。

```rust
/// 単一ドキュメント内で完結するルール(efficiency / consistency / structure)。
/// DocContext が jaio-parse の型を参照するため、trait は jaio-rules に置く
/// (core は jaio-parse に依存しない依存最小 crate であるため)
pub trait DocumentRule: Send + Sync {
    fn id(&self) -> &'static str;
    fn run(&self, ctx: &mut DocContext<'_>) -> anyhow::Result<Vec<Diagnostic>>;
}

/// リポジトリ横断のルール(relation / merge)。Phase 2 で導入
pub trait CorpusRule: Send + Sync {
    fn id(&self) -> &'static str;
    fn run(&self, corpus: &Corpus) -> anyhow::Result<Vec<Diagnostic>>;
}
```

Document ルールはファイル単位で rayon により並列実行します(§10)。Corpus ルールは Index 完了後に実行します。

### 6.2 診断モデル

```rust
pub struct Diagnostic {
    pub rule_id: &'static str,   // "consistency/style-deviation"
    pub severity: Severity,      // Error / Warn / Info
    pub span: Span,              // ファイル内位置(行・列)
    pub message: String,
    pub help: Option<String>,        // 修正方向の提示
    pub evidence: Option<Evidence>, // 根拠(持たない診断もある)
}

pub enum Evidence {
    /// 類似相手との対応(SimHash 候補 + n-gram Jaccard 検証)
    Similarity { target_file: String, target_line: usize, method: &'static str, score: f64 },
    /// トークン削減見積り
    TokenDelta { current: usize, estimated: usize },
    /// 同一出現箇所の一覧(揺れ・逸脱検出用)
    Occurrences { items: Vec<Occurrence> },  // Occurrence = { span, snippet }
    // Graph { detail: GraphDetail } は必要になった時点で追加する
}
```

`evidence` を機械可読として持たせるのが LLM 委譲設計の要です。診断が「なぜ問題と判断されたか」を構造化して伝えます。

### 6.3 Crate 構成

```
jaio/
├── Cargo.toml               # workspace
├── jaio.toml                # 設定(リポジトリルートに配置する運用)
└── crates/
    ├── jaio-core/           # 診断モデル・設定(依存最小)
    ├── jaio-collect/        # ファイル走査(ignore)・コメント抽出(tree-sitter)
    ├── jaio-parse/          # 文書構造 AST(comrak)・日本語 AST(lindera)
    ├── jaio-index/          # 転置索引・SimHash・類似度分布
    ├── jaio-rules/          # ルール trait とルール実装群
    ├── jaio-report/         # 出力(terminal / json / md / sarif)
    └── jaio-cli/            # バイナリ(clap)
```

依存の流れは一方向(cli → report/rules/index/parse/collect → core)とし、循環を許しません。

## 7. 出力仕様

### 7.1 CLI レポート(第一出力)

```
docs/architecture.md:42:5  warning[consistency/style-deviation]  文体の逸脱: ですます体を既定とする文書にである体の文が 3 文あります
  occurrences: L38, L40, L42
  help: ですます体へ統一してください

docs/setup.md:10:3  warning[relation/cross-doc-duplication]  docs/quickstart.md:5 と 78% 類似
  help: 統合候補 — 正候補: quickstart.md §1(被リンク 12 vs 3)
```

文体の統一はトークン削減になるとは限りません(敬体への統一は増加方向のこともあります)。トークン削減見積りは `efficiency` カテゴリのみに付けます。

### 7.2 コマンド体系

```
jaio check [PATH]...     # 解析実行(デフォルトコマンド)
jaio stats [PATH]...     # トークン統計・削減見積りサマリ(--format json で層別集計の自動化、#50)
jaio rules               # ルール一覧
jaio dump doc <PATH>     # 文書構造 AST(leaf・heading・links)の JSON ダンプ
jaio dump morph <PATH>   # 形態素解析結果の JSON ダンプ
jaio dump corpus [PATH]  # Corpus の JSON ダンプ(docs の tokens/body_chars/sections・duplicates・comments・context)
jaio verify-move <before> <after>...  # 文書移動の lossless 検証(#51)
jaio explain <RULE_ID>   # ルールの詳細説明

オプション:
  --format <terminal|json|md|sarif>   # 出力形式(既定: terminal。stats は terminal|json)
  --config <FILE>                     # 設定ファイル(既定: JAIO_CONFIG 環境変数のインライン TOML、
                                      #   なければカレントから上位探索、なければ解析対象パスから探索)
  --no-color / --quiet                # --quiet は error 重大度の診断のみ報告する
```

exit code: `0` = 問題なし、`1` = error レベルの診断あり、`2` = 実行エラーまたは解析未完了(`summary.analysis_complete = false`、§7.3)。

dump 系も `check` と同じ実効設定(形態素解析モード・辞書・トークン encoding・
検出閾値・候補検索方式)を使います(#39)。`dump corpus` の出力には `kind` + `version`
のスキーマ識別を持ち、check と同じ `context`(`config_fingerprint`・`algorithms`・
`targets`・`operational_errors`・`candidate_search`・`sources`)を出します。
運用上の失敗があった dump も部分結果として出力しつつ exit 2 とし、
`analysis_complete: false` で完全性を明示します(check と同じ契約)。
`dump doc` / `dump morph` の構造の正本はそれぞれ
[`docs/schema/dump-doc.schema.json`](schema/dump-doc.schema.json) /
[`docs/schema/dump-morph.schema.json`](schema/dump-morph.schema.json) です(#61)。
`dump corpus` の正本は [`docs/schema/dump-corpus.schema.json`](schema/dump-corpus.schema.json)
(context の構造は report と同じ)です。

`stats --format json` も同じ契約に乗ります(終了コードを含む。読み込み・走査
失敗がある場合は check と同じく `2` を返す): `kind: "jaio/stats"` + `version: 1` の
スキーマ識別、`context`(`config_fingerprint`・`algorithms`・`targets`・
`operational_errors`)を持ち、ファイルごとの `chars` / `tokens` / `tok_per_100ch` と
`total` 集計を JSON で出します。合わせて frontmatter(常時コンテキストに載る層)と
本文(発火時に読まれる層)の `chars` / `tokens` を分離した計測を出します
(frontmatter が無いファイルは `frontmatter: null`、#53)。`--similarity` 併用時は
ヒストグラムと上位ペアも構造化して含めます。コスト評価の再現性は
`config_fingerprint` で管理します。構造の正本は
[`docs/schema/stats.schema.json`](schema/stats.schema.json) です(#61)。

### 7.2.1 `jaio verify-move` — 文書移動の lossless 検証(#51)

文書の「移動のみ・意味保存」リファクタリング(詳細手順の references/ への
原文移動等)で、**移動元の内容が移動先に漏れなく保全されていること**を検証します。
cross-doc-duplication(SimHash)・repeated-quote は「類似」の検出で、完全保全の
検証には使えないため、行単位の集合照合で担います。

- **入力**: `before`(移動元 1 ファイル)と `after`(移動先 1 ファイル以上)
- **正規化**(実リポジトリで運用実績のある照合スクリプトと同じ規則):
  見出しレベルを揃える(`#+` マーカーを除去)、リンク記法 `[text](url)` を
  `text` に簡約(リンク先の実在検証は broken-link が担う)、前後空白の除去。
  画像 `![alt](url)` は簡約せず生比較とする(URL の変更は移動ではなく
  変更のため、#74)。正規化後に空になる行(空白行等)は実質行として数えない
- **判定**: before の正規化済み実質行のうち、全 after を合わせた実質行の
  **多重集合**(同一行は出現回数で消費する)に対応が足りない行を**未保全行**
  として報告する(行番号・正規化後テキスト付き、#80: 同一行の片方だけの
  欠落も検出する)。逆方向(after の回数が before を超える分)は追加行として
  情報表示する
- **出力**: terminal(未保全行リスト)と `--format json`(`kind: "jaio/verify-move"` +
  `version: 1`。構造の正本は [`docs/schema/verify-move.schema.json`](schema/verify-move.schema.json)、#61)。
  exit code は `0` = 完全保全、`1` = 未保全行あり、`2` = 実行エラー
- **限界**: 行単位の一致であって意味の同一性は保証しない(行の並び替え・
  分割は検出しない)。見出し構造の変化は正規化で吸収する
- **check 連動(#56)**: 同じ照合を `relation/move-integrity` ルールとして
  check に組み込める。jaio.toml の `[verification.moves]` で
  `移動先パス = 移動元パス` を宣言すると、check 実行時に自動で検証する。
  移動元を削除した後も宣言を残しておくと「宣言の残骸」として info が出る
  (棚卸しの促し)。verify-move の手動実行が煩になったら宣言に切り替える。
  宣言パスは相対で書ける。解決基準は **① 実効 jaio.toml の配置ディレクトリ
  (運用上 PJ ルート)→ ② 各解析ルート → ③ cwd** の順に実在チェックし、
  最初に実在した基準で絶対化する(#73: cwd 固定だと絶対パス指定の解析で
  誤警告になる)

### 7.3 JSON 出力(`--format json`)

LLM 委譲ワークフロー向けの第一級出力(§2)。構造の正本は JSON Schema
[`docs/schema/report.schema.json`](schema/report.schema.json)(draft 2020-12)。

schema_version 2 では、jaio を「AI が意味判断するための読み取り専用解析器」
とする方針(#31)に合わせ、診断を**測定値 / 位置根拠 / 観察 / 確認事項 / 限界**
に分離します。jaio が決めるのは規約上の severity のみで、意味判断は構造上
AI・人間への委譲として表します。

概形:

```json
{
  "schema_version": 2,
  "tool": { "name": "jaio", "version": "0.1.0" },
  "context": {
    "roots": ["docs/"],
    "config_fingerprint": "sha256:…(実効設定の正規化ハッシュ)",
    "sources": [{ "path": "docs/a.md", "sha256": "…" }],
    "algorithms": {
      "morphology": "lindera-sudachidict(mode=normal, dict=embedded:sudachidict@6.0.0)",
      "tokenizer": "tiktoken o200k_base",
      "similarity": "simhash64+bigram-jaccard",
      "build": "jaio 0.1.0 deps-sha256:…(Cargo.lock のハッシュ)"
    },
    "targets": { "discovered": 5, "analyzed": 4, "skipped": 1, "discovery_complete": true },
    "operational_errors": [
      { "path": "docs/x.md", "stage": "read", "reason": "stream did not contain valid UTF-8" }
    ],
    "rules": [
      { "rule_id": "relation/doc-comment-drift", "status": "insufficient_inputs", "reason": "…" },
      { "rule_id": "structure/section-duplication", "status": "executed", "targets": 33, "measured": 32 }
    ],
    "candidate_search": {
      "method": "simhash_banding", "all_pairs": false,
      "comparable_docs": 2100, "candidates": 1823, "scored_pairs": 1823, "cutoff": 0.65
    }
  },
  "summary": {
    "files": 4, "tokens": 8454,
    "errors": 0, "warnings": 2, "infos": 1,
    "analysis_complete": false,
    "hints": ["orphan が多数検出されています。detection.orphan_entry_points の設定を検討すると検出を絞れます"],
    "top_rules": [["consistency/style-deviation", 1], ["relation/cross-doc-duplication", 1]]
  },
  "diagnostics": [
    {
      "file": "docs/b.md",
      "span": { "line": 1, "col": 1 },
      "rule_id": "relation/cross-doc-duplication",
      "category": "relation",
      "severity": "warn",
      "observation": "docs/a.md と本文の語彙類似度が 0.86 です",
      "measurements": [
        {
          "metric": "bigram_jaccard", "method": "simhash64+bigram-jaccard",
          "status": "measured", "value": 0.86, "unit": "ratio",
          "threshold": 0.65, "comparison": ">=", "scope": "document_pair"
        }
      ],
      "locations": [
        { "file": "docs/a.md", "span": { "line": 1, "col": 1 }, "role": "counterpart", "approximate": false }
      ],
      "review_questions": ["意味的に同一の文書か、版・読者・用途の違いを確認してください"],
      "limitations": ["語彙類似度は意味的同一性を保証しません"]
    }
  ]
}
```

#### 診断フィールド

| フィールド | 内容 |
|---|---|
| `file` / `span` | 主対象の位置(1 基点) |
| `rule_id` / `category` | ルール ID とそのカテゴリ(§5)。ルール意味のバージョンは `measurements[].method` が担う |
| `severity` | 規約上の重大度(`error`/`warn`/`info`)。測定値・確信度・レビュー優先度とは分離する |
| `observation` | 計測値から直接言えること(旧 `message`) |
| `measurements` | 計測値列(下表)。1 診断に複数の測定値を併記できる |
| `locations` | 根拠位置列(下表)。比較両側の位置を含められる |
| `review_questions` | AI・人間が文脈で判断する確認事項(旧 `help`) |
| `limitations` | 解釈上の限界(意味的同一性を保証しない等) |

#### measurement の要素

| フィールド | 内容 |
|---|---|
| `metric` | 指標 ID(ルールカタログ §5 の定義名。例: `bigram_jaccard`, `token_count`, `occurrence_count`) |
| `method` | 算式・算法の識別子とバージョン(例: `bigram-jaccard` / `simhash64+bigram-jaccard`。SimHash を名乗るのは候補列挙が `simhash_banding` のときのみ) |
| `status` | `measured` / `not_applicable` / `insufficient_sample` / `unavailable` |
| `value` | 測定値。**`measured` のとき必須かつ有限**(NaN/Infinity は出力できない)。それ以外の status では **出力しない**。未計測を 0 や NaN/Infinity で埋めない。測定値 0.0 は「計測不能」とは別の正当な値 |
| `unit` | `count` / `ratio` / `tokens` / `chars` 等 |
| `numerator` / `denominator` | 比率の根拠となる分子・分母・標本数 |
| `threshold` / `comparison` | 判定閾値と比較方向(`>=` / `<=` / `<` / `>`) |
| `scope` | 測定の対象範囲(`document_pair` / `leaf` / `corpus` / `symbol:、` 等) |
| `reason` | **`measured` 以外のとき必須**。空文字列不可。計測不能・対象外の理由 |

状態と `value`/`reason` の整合は `docs/schema/report.schema.json` が `if`/`then`/`else` で強制し、Rust 側も `Measurement` のコンストラクタと `JsonReport` 生成時の `validate` が同じ契約を保証する(#32)。

#### 差分系指標の符号

`token_delta` は **`current - candidate`**(現行側のトークン数から候補側を引く)と定義します。正は現行側が大きい=重複側を統合した場合の局所的な削減見込み、負は増加を意味します。実装・スキーマ説明・利用ガイドの 3 点でこの定義を一致させます(#32/#37)。

#### location の要素

| フィールド | 内容 |
|---|---|
| `file` / `span` | 位置。`role = "counterpart"` なら比較相手のファイルを指す |
| `end` | 範囲終端。**正確に取れない場合は出力しない** |
| `snippet` | 該当テキストの断片 |
| `role` | `primary` / `counterpart` / `occurrence` |
| `approximate` | 近似位置なら `true`(形態素・平文化後の位置から正確なソース位置が取れない場合) |

#### 実行コンテキスト(`context`)

同一入力・設定で出力が再現できることを追跡可能にするため、対象ルート・
実効設定の正規化ハッシュ・入力ソースの content hash・前処理/算法/辞書/
トークナイザーの識別情報を出力します。`sources` には解析した文書に加えて
`doc-comment-drift` が実際に読んだコメント抽出元ソースも含め、
「どの入力が結果を構成したか」を照合可能にします(#36)。

`context.algorithms` の識別規則:

| フィールド | 内容 |
|---|---|
| `morphology` | `lindera-sudachidict(mode=…, dict=…)`。辞書実体は埋め込みなら `embedded:sudachidict@<crate 版>`(同梱辞書を一意に決める lindera-sudachidict crate 版。上流 SudachiDict 版は build 時に vendored され実行時には取れない、#64)、外部なら `external:<spec>:manifest-sha256:<hash>`。manifest は lindera が実際にロードする 9 ファイル(`DICTIONARY_FILES`、`package-dict.sh` と同じ集合)の名前・長さ・SHA-256 を束ねたもので、同一パスの差し替えも検出できます。ハッシュを取得できない場合は `unidentified(<理由>)` で、パスだけを内容識別のように見せません |
| `tokenizer` | `tiktoken <encoding>` |
| `similarity` | 候補列挙を含むパイプライン識別(`simhash64+bigram-jaccard`)。実測の算式は各 measurement の `method` |
| `build` | `jaio <version> deps-sha256:<hash>`(ビルド時点のワークスペース `Cargo.lock` の SHA-256)。埋め込み辞書データ・算法・トークナイザーの版追跡はこの共通 manifest への参照で担います |

外部辞書の fingerprint は実行ごとに 1 回だけ解決し、ワーカーの再ロード判定と
出力で共有します。同一プロセスで同じパスの辞書を差し替えた場合は新しい
fingerprint がワーカーキーの不一致として検出され、辞書が再ロードされます。
識別不能な辞書はワーカーを再利用せず毎回ロードします(#36)。

加えて、**不完全な解析をクリーンな結果として誤提示しない**ための状態を
機械可読で出します(#36)。

`context.targets`:

| フィールド | 内容 |
|---|---|
| `discovered` | 走査で発見した対象ファイル数 |
| `analyzed` | 実際に解析まで到達したファイル数 |
| `skipped` | 発見したが読み込み等に失敗してスキップした数 |
| `discovery_complete` | 走査自体が完了したか。走査エラーがあれば `false`(発見数自体が下限) |

`context.operational_errors`: 解析を止めなかった運用上の失敗の列
(`path` / `stage` / `reason`)。`stage` は `scan` / `read` / `metadata` /
`comment_scan` / `comment_read` / `comment_extract` / `rule:<id>`。

`context.rules`: 全ルールの実行状態(`rule_id` / `status` / `reason` /
`targets` / `measured`)。`status` は次のいずれか。

| status | 意味 |
|---|---|
| `executed` | 全対象に対して実行完了 |
| `disabled` | `lint.rules = "allow"` 等で診断が抑制されている |
| `insufficient_inputs` | 入力不足で判定が事実上不可能(比較可能文書 2 件未満等) |
| `failed` | 少なくとも 1 入力で失敗(詳細は `operational_errors`) |

`targets` / `measured` はルール自身が報告した**適用範囲**で、診断数とは独立に
「対象がなかった」「計測できなかった」を表します(#36)。`targets: 0` は適用対象
なし(正常な 0 件)、`targets > 0 かつ measured: 0` は対象があったが判定不能、
`targets > measured` は部分カバレッジを意味します。未実行・disabled のルール
では出力しません。

`suppressed_by_path` は `[lint.rule_excludes]` のパス照合で抑制された診断数です
(#82)。ルールは executed のまま、診断の一部が設定で抑えられたことを文脈から
読み取れるようにします。抑制がなければ出力しません。

抑制されたルール(`disabled`)は実行自体をスキップし、ルール専用の入力取得
(コメント抽出等)も行いません。実行前に有効ルール集合を確定し、無効ルール用の
入力コスト・エラーを発生させません(#36)。

`context.candidate_search`: 文書重複の候補列挙方式(#36)。

| フィールド | 内容 |
|---|---|
| `method` | `all_pairs` / `simhash_banding` |
| `all_pairs` | 全ペア採点だったか(`detection.all_pairs` 指定時 `true`) |
| `comparable_docs` | 比較対象となった文書数(本文長フィルタ適用後) |
| `candidates` | 列挙された候補ペア数 |
| `scored_pairs` | 実際に類似度を採点したペア数 |
| `cutoff` | 採点に使った類似度の下限(`detection.doc_similarity` と `merge.min_similarity` の低い方。各ルールは自身の閾値でさらに絞る) |

`simhash_banding` は近似検索であり、未検出は「全ペアを調べて類似なし」と
同義ではありません。この場合、重複系診断の `limitations` にもその旨を
付記し、`[detection] all_pairs = true` で全ペア採点に切り替えられます。

`summary.analysis_complete`: 上記の走査・読み込み・ルール実行が全て
完了したか。`false` のとき終了コードは 2(実行エラー扱い)で、
「error 診断なし」でもクリーン扱いしません。

`summary.hints`: 設定による緩和が見込める大量検出への案内(#91)。
orphan が 20 件超かつ解析成功文書数の 30% 超など、設定不足の可能性が
高いときに 1 行案内を入れる。該当なしのときは空配列。診断ではなく
実行側の助言のため、`diagnostics` には入れない(重大度を持たせない)。

#### v1 → v2 対応表

| v1 | v2 | 変更理由 |
|---|---|---|
| `message` | `observation` | 観察と判断の分離 |
| `help` | `review_questions[]` | 修正指示ではなく非拘束の確認事項へ |
| `evidence.token_delta` | `measurements`(`token_count`×2 + `token_delta`) | 複数測定値の併記・符号付き差分 |
| `evidence.occurrences` | `locations`(`role=occurrence`、`snippet` 維持) | 位置根拠の一般化 |
| `evidence.similarity` | `measurements`(`bigram_jaccard`)+`locations`(`role=counterpart`) | 測定値と位置の分離 |
| `fixable` / `summary.fixes` | 削除 | 自動修正廃止(§7.4、#33)。古い意味で false/0 を返し続けない |
| `summary.estimated_reduction` | 削除 | 異種の推計を単純合算しない(#34 で測定値として再定義) |

- `schema_version` は出力構造のバージョン。同じ値の間は既存フィールドの削除・型変更を行わず、追加のみ許す。v2 は v1 と互換性がないため増やした
- `diagnostics` は file → line → col → rule_id → observation の順で決定的に並ぶ
- `doc-comment-drift` の意味レベルの乖離判定は jaio では行わない。比較方式・類似度・両側の位置を材料として渡し、どちらが古いかの判断は LLM 側に委譲する

### 7.4 読み取り専用方針(旧 `--fix` は廃止)

**jaio は解析対象の文書を一切変更しません**(§31 の方針、#33)。置き換え候補の
提示は非拘束的な `review_questions`・`measurements` の材料として残しますが、
適用の決定も実行も行いません。

旧 `jaio check --fix` は廃止しました。`--fix` を指定した場合は未知の引数として
終了コード 2 で止まり、対象ファイルは変更されません。診断位置の近似や
leaf.text に含まれない記法部分(リンク URL・autolink・参照定義・インライン
コード)の扱いは、書き換えではなく位置根拠の精度(`locations[].approximate`)
として残ります。

非拘束的な候補として残るもの:

- 冗長表現パターンの置き換え候補と局所的なトークン差分(指定 encoding での
  比較。編集後の文書全体の実測とは別指標で、符号を保存し単純合算しない)
- 用語揺れの推奨表記候補(文脈による採否が必要な提案として)
- 句読点の多数派表記(和文文脈の判定条件つき)

### 7.5 LLM 委譲ワークフロー(Issue #7)

jaio の診断を LLM エージェントが消費する標準フローを次に定めます。

**入力**:

1. `jaio check <paths> --format json` — 診断とサマリ。第一の入力。
   まず `summary.analysis_complete` と `context.rules` で解析の完全性を確認する
2. `jaio dump corpus <paths>` — 重複の調査・統合判断の追加文脈(本文全文は含まないため、必要に応じて対象ファイルを直接読む)
3. `jaio dump doc / morph <file>` — 個別診断の根拠確認(誤検出の疑いがあるとき)

**判断の型**(詳細は `docs/llm-usage.md`): 各診断に対し LLM/人間は
**変更**(編集して再実行で確認)・**維持**(正当な重複・意図的表現として
扱う)・**保留**(根拠不足のため dump やファイル閲覧で追加調査)のいずれかを
判断します。スコア・severity・0 件検出はいずれも自動的な指示ではありません(#37)。

**委譲点(jaio は決定しない)**:

| ルール | LLM/人間が判断する内容 |
|---|---|
| `merge/consolidation-plan` | 統合の実行可否・正候補の最終選択 |
| `relation/cross-doc-duplication` | 類似の実害(正当な重複か、統合すべきか) |
| `relation/doc-comment-drift` | 意味レベルの乖離(文字 bigram では検出不能、§5.4) |
| `efficiency/token-density` | 密度が低いことの文脈適否 |
| すべての info | 報告の要否(プロジェクトの慣例による) |

**出力の安定性**:

- `summary`・`diagnostics` のキーは**追加のみ可**(既存キーの削除・改名・意味変更はしない)。
  ただし `required` への追加は「旧版の出力が新 schema で違反になる」ことを意味するため、
  schema の保証対象は「schema と同じ版の jaio の出力」である。旧出力との照合には
  出力側の `tool.version` か対応する版の schema を使う
- `measurements`・`locations` の要素列挙(指標 ID・ロール等)は追加のみ可
- この方針により LLM 側のプロンプトが jaio の更新で壊れない

**プロンプトの要点**(LLM ガイドの全文は `docs/llm-usage.md`):

- `review_questions` は確認事項であり、実行すべき手順ではない
- `measurements`・`locations` を根拠として扱い、`occurrence` の `snippet` と行番号で対象箇所を特定する
- 修正後は再実行して診断の解消を確認する

## 8. 設定仕様(`jaio.toml`)

```toml
# 形態素解析・トークン計測
[analysis]
mode = "normal"             # lindera Mode: "normal" | "decompose"
encoding = "o200k_base"     # トークン計測用エンコーディング
# dictionary = "/path/to/lindera-sudachidict"  # 外部の lindera 辞書ディレクトリ(Sudachi スキーマ)。未指定なら埋め込み辞書

# 文体(§1.3 の既定規約)。target = "polite"(ですます体)| "plain"(である体)
[style]
target = "polite"

# 文書単位の文体上書き(#55)。パスパターン(gitignore 構文)→ 文体。
# frontmatter の jaio-style 宣言(jaio-style: polite 等)はこれより優先する。
# パターン系設定の照合は、実効 jaio.toml の配置ディレクトリ基準で相対化した
# パスで行う(絶対パス実行・サブディレクトリを解析ルートにした実行でも
# 一貫して効く、#76)
[style.overrides]
"docs/templates/**" = "polite"
"**/SKILL.md" = "plain"

# 対象パス
[paths]
include = ["docs/**/*.md", "**/*.md"]  # 収集対象の glob(gitignore 構文)。空なら全件
exclude = ["target/", "node_modules/"] # gitignore 完全準拠(ネガーション `!`・`**` 対応)
include_hidden = true                  # ドット始まりの隠しエントリ(.agents/ 等)を対象に含める。エージェント運用
                                       # リポジトリでは隠しディレクトリが主要な文書置き場になるため既定は true。
                                       # `.git` はこの設定によらず常に除外する

# ソースコードのコメント抽出(§5.4 の doc-comment-drift 入力)
[comments]
languages = ["rust", "typescript", "python", "go"]  # 抽出対象言語。空なら全言語

# 統合提案
[merge]
min_similarity = 0.65        # 統合候補とする最低類似度(非公開の実運用測定に基づく。0.75 は正当な重複を見逃すため)
                             # 重複の報告閾値は detection.doc_similarity が担う

[merge.weights]             # 正候補スコアの重み
inbound_links = 1.0
recency = 0.0                  # 新しさは opt-in(>0 で有効化)。既定では mtime を使わない
role = 0.8

# 検出の感度パラメータ(チューニング対象。既定値はコード内定数と同じ)
[detection]
doc_body_min_chars = 50      # 文書重複の比較対象とする本文の最小文字数(正規化後)
particle_pairs = []         # particle の報告助詞ペア(opt-in。例: [["に","へ"]])
doc_similarity = 0.65        # 文書重複として報告する類似度(統合提案の閾値とは別)
comment_min_chars = 40       # doc-comment-drift の対象とするコメント本文の最小文字数(正規化後)
comment_similarity = 0.6     # doc-comment-drift の重複判定類似度
section_similarity = 0.75    # セクション重複判定類似度
section_body_min_chars = 30  # セクション重複の比較対象とする本文の最小文字数(正規化後)
repeat_quote_min_chars = 30  # 同一文の重複出現として報告する文の最小文字数(正規化後)
long_sentence_min_chars = 100 # 長い文として報告する和文文字数(ASCII は数えない、#48)
no_comma_min_chars = 60       # 読点なし長文として報告する和文文字数(#48)
max_nest_depth = 3           # リスト・引用のネストとして報告する深さ(この深さを超えるもの)
orphan_entry_points = ["README.md"]  # orphan 判定から除くエントリポイント(gitignore 構文。`/` なしは任意階層のファイル名に一致。
                                     # 照合は jaio.toml 配置ディレクトリ基準の相対パス、#76)

# 重大度制御(rustfmt 風)
[lint]
default = "warn"

[lint.rules]
"consistency/style-deviation" = "deny"

# ルール別の適用除外パス(gitignore 構文、#75)。テンプレート文書の見出し枠など
# 「特定パスでは正当」な診断を抑える。[lint.rules] の allow(全パスで抑制)と
# 違い、対象外のパスには通常どおり報告される。照合は jaio.toml 配置ディレクトリ
# 基準の相対パス(#76)
[lint.rule_excludes]
"structure/empty-section" = [".opencode/commands/**"]

# 意図的な揺れの許容
[lint.allow]
terms = ["リポジトリ", "レポジトリ"]

# 文書移動の保全検証(#56)。移動先 = 移動元 の宣言で、check 実行時に
# relation/move-integrity が行単位の保全を自動検証する。移動元を削除したら
# 宣言も削除する(残ると info が出る)。宣言パスの相対基準は
# ① この jaio.toml の配置ディレクトリ → ② 解析ルート → ③ cwd の順に
# 実在チェックする(#73)
[verification.moves]
# "docs/new-location.md" = "docs/old-location.md"

# ユーザー拡張の冗長表現パターン
[patterns.redundant]
"することが可能です" = "できます"
```

## 9. 技術選定

| 領域 | 選択 | 備考 |
|---|---|---|
| Markdown 解析 | `comrak` | GFM 対応、MDAST 構造の取得 |
| 形態素解析 | `lindera`(Sudachi 辞書) | Rust ネイティブ。v6 の `embed-sudachidict` feature で辞書埋め込み・オフライン動作(jaio の Cargo feature `embedded-dict`、既定 on)。`--no-default-features` の軽量ビルドでは `[analysis] dictionary` で外部辞書ディレクトリを実行時ロード(mmap)する。粒度 A/B/C は `Mode` 非提供(§4.2) |
| トークン計測 | `tiktoken-rs` | BPE エンコーディング指定可 |
| コメント抽出 | `tree-sitter` + 各言語 grammar | ビルド負荷のため feature flag で主要言語から段階提供。Phase 2 では Rust / TypeScript / Python / Go の comment ノード抽出 |
| ファイル走査 | `ignore` | ripgrep 由来、.gitignore 準拠(`require_git(false)`:`.git` の有無に依らず .gitignore を適用、#58) |
| 並列処理 | `rayon` | ファイル単位の並列パース |
| CLI | `clap` | derive API |
| 直列化 | `serde` / `serde_json` | 設定・診断 JSON |

## 10. パフォーマンス設計

- ファイル単位の `rayon` 並列(読み込み・パース・DocumentRule 実行)。
  Analyzer/TokenCounter はスレッドごとに持ち(失敗もキーつきでキャッシュし
  同一キーでの初期化再試行を防ぐ、#44)、収集順序はファイル順で保持する
  (診断の出力順は最終ソートで決定的)
- Corpus 構築のペア採点(SimHash 候補 × bigram Jaccard)も rayon で並列化。
  ペア列挙は逐次のまま(候補生成自体は軽いため)
- 形態素解析結果の文字列単位キャッシュは**ファイル単位**で作る(#40:
  スレッド永続キャッシュは、一意文の多い大規模コーパスでメモリが入力の
  約 170 倍に膨らむ実測があった。同一見出し・頻出句の再解析回避という
  趣旨はファイル内の再利用でほぼ達成できる)
- インクリメンタル解析(mtime + ハッシュによる再解析スキップ)は Phase 3。
  実測ではファイル単位の処理が 500 文書で約 2ms/文書に留まるため、
  永続キャッシュ化より並列化を優先した(Issue #14)
- 辞書・パターン辞書は `include_str!` / `include_bytes!` で静的アセット化
- 計測(Issue #18): check 実行のフェーズ別(collect / parse / corpus)所要時間とルール別の診断数・所要時間を `--verbose` で summary に出す。性能の議論を推定から実測に変える

## 11. ロードマップ

### Phase 1 — 単一ドキュメント内解析(完了)

- [x] Cargo workspace 骨格、品質ゲート(ローカル `check-all.sh` による fmt / clippy / test。CI 不使用運用は `AGENTS.md` 参照)
- [x] comrak による文書構造 AST 構築
- [x] lindera + Sudachi による形態素解析・日本語 AST
- [x] tiktoken-rs によるトークン計測
- [x] ルール: `efficiency/redundant-expression`(フィラーを含む)、`consistency/notation-variant`、`consistency/style-deviation`、`consistency/particle`、`structure/heading-skip`、`structure/empty-section`、`structure/over-nesting`、`structure/section-duplication`
- [x] CLI レポート(terminal)+ `jaio stats`
- [x] `jaio.toml` 最小実装(`paths.exclude`、`lint.rules` の重大度上書き・allow 抑制)
- [x] 形態素解析のキャッシュ(テキスト単位、§10)

完了条件: サンプル文書群に対して基本ルールが正しく診断を出すこと。`cargo test` で各ルールのユニットテストが通ること。

### Phase 2 — リポジトリ横断解析

- [x] JSON 出力(`--format json`、§7.3)
- [x] `jaio rules` / `jaio explain`
- [x] Markdown リンク抽出とファイル参照集合の構築
- [x] ルール: `relation/broken-link`、`relation/orphan`
- [x] `jaio-index` crate の分離(SimHash・n-gram 類似度・テキスト正規化)
- [x] SimHash + n-gram による重複検出(小規模コーパスは全ペア検証で擬陰性を回避)
- [x] 重複位置の特定(最も類似するセクション組を見出し行で報告)
- [x] ルール: `relation/cross-doc-duplication`、`merge/consolidation-plan`(§5.5)
- [x] `jaio-collect` crate の分離(tree-sitter によるコメント抽出)
- [x] ルール: `relation/doc-comment-drift`(コメントと文書の重複)
- [x] ルール: `consistency/terminology`(ペア辞書ベースの用語揺れ、`[terms]` 拡張)
- [x] ルール: `efficiency/repeated-quote`(同一文の重複出現)
- [x] 解析の並列化(rayon: ファイル単位のパース+ルール実行、Corpus 構築のペア採点。Issue #14)
- [x] ルール: `relation/dir-mismatch`
- [x] `jaio.toml` 主要項目(style.target、`[patterns.redundant]` ユーザー拡張、`[lint.allow]` 許容表記)
- [x] `jaio.toml` 残項目(gitignore 完全準拠の exclude、`[paths] include`、`[analysis]`、`[comments]` コメント対象言語、`[lint] default`)

### Phase 3 — 修正支援と運用

- [x] ~~`--fix`~~ → 読み取り専用方針に転換し廃止(§7.4、#33)
- [x] ~~インクリメンタル解析~~ → 実測で効果対コストが見合わず見送り、rayon 並列化で代替(#14)
- [x] SARIF 出力
- [x] LLM 委譲ワークフロー向けの抽出データ整備(JSON スキーマ確定: `docs/schema/report.schema.json`、§7.3)

## 12. リスクと緩和策

| リスク | 影響 | 緩和策 |
|---|---|---|
| lindera の Sudachi 辞書対応 | Phase 1 の前提 | 検証済み: v6.0 `embed-sudachidict` で利用可。ただし Sudachi 粒度 A/B/C 相当の Mode は無く `Normal`/`Decompose` のみ(§4.2)。粒度の細かい制御が必要なら将来 vibrato 等へ差し替え(分析 trait で吸収) |
| 埋め込み辞書によるバイナリ肥大(約 600MB、`dict.words` が 400MB) | 配布の障壁 | 既定ビルドは埋め込みのまま(オフライン・設定不要を優先)。配布用は Cargo feature `embedded-dict` を外し、`[analysis] dictionary` で外部辞書(`lindera build` で SudachiDict small/core から作ったものも可)を実行時ロードする。外部辞書は Sudachi スキーマ(素性列の並び)を前提とし、IPADIC 等への切り替えは素性参照とルールの見直しが要るため行わない |
| tree-sitter grammar のビルド負荷 | 導入障壁 | feature flag で言語を段階提供。コメント抽出のみなら簡易パーサ(`//` `#` `/* */`)へのフォールバックも保持 |
| SimHash の誤検出 | 統合提案の信頼低下 | しきい値を設定可能にし、`evidence` に根拠(スコア・一致位置)を必ず出す。最終判断は LLM/人間 |
| NFKC 正規化による過剰検出 | 揺れ検出の誤報 | 表記比較は NFC、全角半角統一は明示的ルールでのみ実施 |
| 形態素解析の解析漏れ・誤分割 | てにをは検出の精度 | 格の意味役荷判定は行わない設計(§5.2)。報告ペアを限定して誤検出を抑制し、`consistency/particle` は info で運用する |
| 敬体規約とトークン効率のトレードオフ | 効率ルールと文体規約の矛盾 | 効率ルールの提案は目標文体を保存する(§5.1)。文体統一のトークン影響は見積りに出さない |

## 13. 用語集

| 用語 | 定義 |
|---|---|
| 文節(チャンク) | 形態素列を区切った、係り受けの最小単位 |
| 原形(lemma) | 形態素解析が返す辞書見出し語。揺れ検出のグルーピングキー |
| SimHash | 局所敏感性ハッシュ。類似文書の候補ペア抽出に使用 |
| 正候補 | 統合時に残す側の文書。被リンク数・新しさ・役割でスコア化 |
| Document ルール / Corpus ルール | 単一ドキュメント内で完結するルール / リポジトリ横断のルール |
| 敬体・常体 | ですます体 / である体(§1.3 の既定は敬体) |

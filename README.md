# jaio

リポジトリ内の日本語文書を静的解析し、**人間の可読性**と **AI のトークン効率**の両面から冗長性・揺れを検出する Rust 製 CLI ツールです。

「文書版 clippy」を目指します。診断は Rust の linter 文化(warn/deny、rule ID)に倣い、観察・計測値・位置根拠・確認事項・限界を分離して報告します。文書の編集は行いません(読み取り専用、設計書 §7.4)。

リリース運用(定期サイクル・手順)は [docs/release.md](docs/release.md) を参照してください。

## 使い方

```sh
cargo run -p jaio-cli -- check docs/            # ドキュメントを解析して診断を報告
cargo run -p jaio-cli -- check docs/ --format json  # JSON 出力(LLM 委譲向け、§7.3)
#   JSON の構造は docs/schema/report.schema.json(schema_version で互換性管理)
cargo run -p jaio-cli -- check docs/ --verbose  # フェーズ別・ルール別の処理時間など内部計測を表示
cargo run -p jaio-cli -- stats docs/            # トークン統計を表示
cargo run -p jaio-cli -- stats docs/ --format json  # トークン統計の JSON 出力(層別集計の自動化向け、#50)
cargo run -p jaio-cli -- stats docs/ --similarity  # 文書ペアの類似度分布(重複閾値の調整用)
cargo run -p jaio-cli -- dump doc <file>        # 文書構造 AST(leaf・heading・links)を JSON でダンプ
cargo run -p jaio-cli -- dump morph <file>      # 形態素解析結果を JSON でダンプ
cargo run -p jaio-cli -- dump corpus docs/      # 文書別統計・重複ペア等を JSON でダンプ(重複調査・層別分析の材料)
cargo run -p jaio-cli -- verify-move OLD.md NEW.md  # 文書移動の lossless 検証(行単位保全、#51)
cargo run -p jaio-cli -- rules                  # ルール一覧
cargo run -p jaio-cli -- explain <RULE_ID>      # ルールの詳細
```

既定ビルドは Sudachi 辞書を埋め込むためバイナリが約 600MB になります。配布用には `cargo build -p jaio-cli --release --no-default-features` で軽量ビルド(約 15MB)を作り、`jaio.toml` の `[analysis] dictionary` に lindera 辞書ディレクトリを指定して実行時ロードします(設計書 §12)。

## リリース

リリースを 1 コマンドで作成します。運用の正本は [docs/release.md](docs/release.md) を参照してください:

```sh
./scripts/release.sh              # v<version> を作成(binary + dict)
./scripts/release.sh 0.2.0        # バージョン指定
./scripts/release.sh --with-dict  # 辞書同梱版も追加
```

**リリースは変更内容で機械的に判断**します(feat/fix を push したらリリース、
日数は使いません。詳細は [docs/release.md](docs/release.md)):

バイナリの取得:

```sh
gh release download v<version> -R oboro-app/oboro-ja
tar xzf jaio-v*-*.tar.gz && tar xzf jaio-dict-*.tar.gz
./jaio check . --config jaio.toml   # 辞書パスを [analysis] dictionary に指定
```

設定は `jaio.toml`(リポジトリルート)で調整します:

```toml
[style]
target = "polite"        # 既定の文体(polite = ですます体 / plain = である体)

[patterns.redundant]     # 冗長表現パターンの追加(表現 = 置き換え先)
"検討を行う必要がございます" = "検討が必要です"

[lint.allow]             # 意図的な揺れの許容
terms = ["行なう"]

[merge]                  # 統合提案・重複検出
min_similarity = 0.65

[detection]              # 検出感度(調整は scripts/eval-detection.py・eval-rules.py で検証)
doc_body_min_chars = 50
orphan_entry_points = ["README.md"]  # orphan 判定から除くエントリポイント(gitignore 構文)
```

出力例:

```
docs/setup.md:7:15  warning[efficiency/redundant-expression]  冗長表現: 「することができます」→「できます」
  question: 「できます」に短縮しても意味・敬意が変わらないか文脈を確認してください
  measurement: token_delta=5 tokens (local_encoding_count, pattern)
docs/setup.md:3:1  warning[consistency/style-deviation]  文体の逸脱: ですます体を既定とする文書にである体の文が 3 文あります
  occurrences: L3「例示の断片」, L20「二つ目」, L26「三つ目」
```

実装済みのルール(19 個、`jaio rules` で一覧できます):

| カテゴリ | ルール | 検出 |
|---|---|---|
| `efficiency` | `redundant-expression` | 定型冗長表現(置き換え候補、トークン削減見積り付き) |
| `efficiency` | `filler` | フィラー句・空虚修飾(削除候補) |
| `efficiency` | `token-density` | 情報密度の低い段落(コンテンツ語比率の統計的外れ値) |
| `efficiency` | `repeated-quote` | 同一文の重複出現(引用コピペ等) |
| `consistency` | `style-deviation` | ですます体を既定とした文体の逸脱(常体優勢の文書は文書レベルの info) |
| `consistency` | `notation-variant` | 同一読みの語の表記ゆれ(行う/行なう 等) |
| `consistency` | `terminology` | 用語揺れ(サーバー/サーバ 等、ペア辞書 + `[terms]` 拡張) |
| `consistency` | `particle` | 同一述語に対する格助詞の揺れ(てにをは) |
| `consistency` | `punctuation` | 句読点の表記揺れ(和文句読点・全角異体・和文文脈の ASCII 記号の混在) |
| `structure` | `heading-skip` / `empty-section` | 見出しレベルの跳び、本文のない見出し |
| `structure` | `over-nesting` / `section-duplication` | 過剰なネスト、文書内の重複セクション |
| `relation` | `broken-link` / `orphan` | リンク切れ、参照されない孤立文書 |
| `relation` | `cross-doc-duplication` / `doc-comment-drift` | 文書間重複(SimHash + Jaccard、重複セクション位置つき)、コメントと文書の重複 |
| `relation` | `dir-mismatch` | ディレクトリ役割(README)と文書内容の不一致 |
| `merge` | `consolidation-plan` | 重複グループの統合提案(正候補スコアとトークン削減見積り) |

## 特徴

- **3 層 AST**: Markdown 構造 AST(comrak)× 日本語 AST(lindera + Sudachi 辞書)× リポジトリグラフ(リンク・コメント・ディレクトリ)
- **冗長性の検出**: 定型冗長表現、文書内・文書間重複(重複位置つき)、コメント↔文書の重複
- **揺れの検出**: 表記ゆれ、用語揺れ、文体の逸脱、てにをは、句読点
- **統合提案**: 重複セクションの統合候補と正候補スコアを「材料」として提示(判断は人間/LLM に委譲)
- **トークン効率**: tiktoken(o200k_base)による実測と削減見積り

## ポジショニング

jaio は**決定論的な静的解析のみ**を行います。意味的な冗長性の最終判断(本当に統合すべきか、どう書き直すべきか)は組み込まず、判断に必要な構造化データを CLI レポートおよび JSON(Phase 2)で提供します。LLM エージェントが jaio を実行して JSON を読み、意味判断と編集をするワークフローを想定しています。

本ツールが対象リポジトリに求める既定の文体は「丁寧(ですます体)かつ無駄のない文章」です(設計書 §1.3)。

## ステータス

設計と実装の正本は [docs/design.md](docs/design.md)、運用は [AGENTS.md](AGENTS.md) を参照してください。未解決事項は [GitHub Issues](https://github.com/oboro-app/oboro-ja/issues) で管理します。LLM エージェント向けの利用ガイドは [docs/llm-usage.md](docs/llm-usage.md) にあります。

| Phase | スコープ | 状態 |
|---|---|---|
| 1 | 単一ドキュメント内解析(効率・揺れ・構造ルール) | 実装済み |
| 2 | リポジトリ横断解析(重複・関係・統合提案、`jaio.toml`) | 実装済み |
| 3 | SARIF 出力、インクリメンタル解析 | SARIF・md・quiet 出力は実装済み。`--fix` は読み取り専用方針へ転換して廃止(§7.4)。インクリメンタル解析は未着手 |

## ユースケース別の設定例

汎用ドキュメント以外のリポジトリでは、既定の設定で指摘が大量に出る(または出すぎない)
ことがあります。jaio.toml(スキーマは [docs/design.md](docs/design.md) §8)で
次のように調整できます。いずれもエージェント指示ドキュメント中心のリポジトリでの
ドッグフーディング(エージェント指示文書が中心の実リポジトリ)で実績のある形です。

しきい値調整の根拠は評価器で計測します。文・段落レベルの効率・揺れ系ルールは
`scripts/eval-rules.py`、文書ペア重複は `scripts/eval-detection.py` が、
ラベル付き合成コーパスで再現率・偽陽性・未判定検出を報告します(#62)。
実リポジトリ分布の回帰は `scripts/eval-field.py` が実データの cross-doc
ペア集合をベースライン照合で検証します(#70。解析対象のパスはコマンドライン
で渡し、実ファイルは jaio リポジトリに含めない。ベースラインは
`scripts/field-baselines/`。検出挙動を意図的に変えたら `--update-baseline`):

```sh
./scripts/eval-field.py --repo /path/to/repo --name sample \
    --set analysis.dictionary=/path/to/lindera-sudachidict
```

### エージェント指示ドキュメント(AGENTS.md / SKILL.md 等)

```toml
# 指示文書はである体。jaio 既定(ですます体)のままでは文体逸脱が大量に出る
[style]
target = "plain"

[paths]
# .agents/ 等の隠しディレクトリが主要な文書置き場になるため対象に含める。
# .git はこの設定によらず常に除外される
include_hidden = true

[detection]
# ハーネス(Claude Code / OpenCode 等)が名前規約で発見するファイルは
# 文書間リンクされないため、orphan の対象外にする
orphan_entry_points = [
  "README.md",
  "AGENTS.md",
  "CLAUDE.md",
  "SKILL.md",       # `/` なしのパターンは任意階層のファイル名に一致
]
```

### テンプレート・素材を含むリポジトリ(フォーク前提の boilerplate 等)

```toml
[paths]
exclude = [
  # フォーク後に利用者が埋める空テンプレート。未実装を示す空リンクは
  # broken-link の正当な指摘ではなくテンプレートの仕様なので除外する
  "skill-packs/*/templates/",
]

# 除外したくないが「特定パスでは正当」な指摘は、ルール別の適用除外パスで
# 抑制する(#75)。対象外のパスには通常どおり報告される
[lint.rule_excludes]
# 生成時に埋める見出し枠を持つテンプレート文書の空セクション
"structure/empty-section" = [".opencode/commands/**"]
```

素材・テンプレートのうち「意図的な前方参照(未実装・未ロックを表すリンク切れ)」は
exclude で除外するか、指摘を維持判断(放置)として運用します。

### 文書単位の文体宣言(ですます体が正当な素材・テンプレート)

リポジトリ全体はである体(`[style] target = "plain"`)でも、報告書テンプレートや
レビュープロンプト素材など敬体が正当な文書があります。**文書の frontmatter に
宣言する**と、その文書だけ敬体として判定されます(#55):

```markdown
---
jaio-style: polite
---
```

文書を編集したくないもの(配布テンプレート等)は jaio.toml のパスパターンで
宣言します(gitignore 構文):

```toml
[style.overrides]
"docs/templates/**" = "polite"
```

優先順位は frontmatter の `jaio-style` > `[style.overrides]` > `[style] target` です。

### SKILL.md から素材を参照するスキル規約

SKILL.md から `references/` 配下を Markdown リンク(`[詳細](references/x.md)`)
ではなくパス文字列で参照する規約の場合、素材ファイルはリンクグラフに乗らないため
orphan として報告されます。次のいずれかを選択します:

1. 参照を Markdown リンクにする(素材がリンクグラフに乗る)
2. 素材ファイル(または素材ディレクトリ)を `orphan_entry_points` や exclude で対象外にする

### 管理された複製の監視(ハーネス横断の定義複製)

同一ファイルの複製(`.claude/` と `.cursor/` に同じ本文、等)は
`relation/cross-doc-duplication` と `merge/consolidation-plan` が重複位置つきで報告します。
「model 記述だけがハーネス別に異なる管理された複製」のように維持を決めたものは、
consolidation-plan を定期監視(新規複製の検知)に使う運用ができます。

## ビルドと辞書の選択

| ビルド | バイナリサイズ | 初回起動 | 辞書 |
|---|---|---|---|
| 既定(`cargo build --release`) | 約 584MB | コールドで約 30 秒(ページイン) | SudachiDict 埋め込み |
| `--no-default-features` | **約 15MB** | 一瞬 | 外部辞書(`[analysis] dictionary`) |

小さい配布物や高速起動が必要な場合は `--no-default-features` でビルドし、
`jaio.toml` で辞書ディレクトリを指定します:

```toml
[analysis]
# 絶対パスか file:// URI を推奨(相対パスは実行時 cwd 基準になる)
dictionary = "/path/to/lindera-sudachidict"
```

外部辞書ディレクトリは lindera 形式(`dict.trie`、`matrix.mtx`、`metadata.json` 等)。
埋め込みビルドの `target/<profile>/build/lindera-sudachidict-*/out/lindera-sudachidict`
にある成果物をそのまま利用できます(実機検証済み)。

辞書の配布手順(Issue #27):

```sh
./scripts/package-dict.sh                      # ビルド成果物を自動検出して tar.gz + .sha256 を生成
./scripts/package-dict.sh /path/to/dict -o sudachidict.tar.gz  # lindera-cli 自前ビルドの辞書も可
```

リリースバイナリ(辞書なし + 辞書パッケージ)は
[GitHub Releases](https://github.com/oboro-app/oboro-ja/releases) から取得できます
(`gh release download -R oboro-app/oboro-ja`)。リリース運用の詳細は
[docs/release.md](docs/release.md) を参照してください。
なお `--no-default-features` ビルドは形態素解析を省略できるものではなく、
`check`/`stats`/`dump` の実行には `[analysis] dictionary` が必須です
(辞書なしで動くのは `rules`/`explain` のみ)。

### 辞書の更新と再検証

使用した辞書は出力の `context.algorithms.morphology` の `dict=` で識別されます
(`config_fingerprint` ではなくこちらが辞書実体の記録です):

- 埋め込み: `embedded:sudachidict@<crate 版>` — lindera-sudachidict crate 版が
  同梱辞書を一意に決めます(上流 SudachiDict 版は build 時に vendored され、
  実行時には取得できません)
- 外部: `external:<spec>:manifest-sha256:<hash>` — lindera がロードする
  9 ファイルの内容 fingerprint。上流版は辞書ディレクトリに記録されないため、
  このハッシュが実質の版識別です

辞書を更新すると形態素の分割・品詞・読みが変わり得ます。**影響を受けるのは
形態素を使うルールのみ**です:

| 影響あり(形態素ベース) | 影響なし(テキスト・構造ベース) |
|---|---|
| `efficiency/token-density`(コンテンツ語比率)、`consistency/style-deviation`(文の文体判定)、`consistency/particle`(格助詞)、`consistency/notation-variant`(読み・正規化形の比較) | `efficiency` の他ルール、`consistency/terminology`、`consistency/punctuation`、`structure/*`、`relation/*`、`merge/*`、トークン計測(tiktoken) |

辞書に無い語(新語・サービス名等)は未知語として粗い品詞推定になり、
左列ルールの計測が変わる可能性があります(例: 未知語の読みが取れず
notation-variant の比較対象から外れる)。

更新時の再検証手順:

1. `./scripts/eval-rules.py`(形態素依存の文レベルルール)と
   `./scripts/eval-detection.py`(文書ペア重複)でグラウンドトゥルース付き
   コーパスの再現率・偽陽性を更新前後で比較
2. 既知のリポジトリで `check --format json` を取り診断の差分を確認
   (`config_fingerprint` が同じで `dict=` だけ変わる差分は辞書由来)
3. 気になる文は `dump morph <file>` で分割・品詞の変化を直接確認

差し替えの目安: 定期更新の必要はなく、対象リポジトリの新語・固有名詞で
検出漏れが疑われるときや lindera 依存の更新に追従するときに行います。
辞書を変えると `dict=` 識別子が変わり過去レポートと直接比較できなくなるため、
理由のあるタイミングで行ってください。

ライセンス: SudachiDict は Apache License 2.0 で再配布可能です
(ライセンス条文と Works Applications の著作権表示の同梱が条件)。
辞書 tar.gz や辞書同梱バイナリを配布する場合はライセンス文を同梱してください。

## ライセンス

本リポジトリは非公開で、**自社ライセンス**の下で利用します(`Cargo.toml` は `UNLICENSED`)。
自社ライセンスの条文は別途定めるものとし、必要になった時点で `LICENSE` ファイルとして配置します。
公開する場合は SPDX ライセンスを選定し直します。

## ライセンス

MIT License ([LICENSE-MIT](LICENSE-MIT)) と Apache License 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) のどちらかを選んで利用できます。

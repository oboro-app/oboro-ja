# docs 読み順・規約

| 知りたいこと | 参照先 |
|---|---|
| 設計の正本(目的・アーキテクチャ・ルールカタログ・ロードマップ) | `docs/design.md` |
| LLM エージェント向けの利用ガイド | `docs/llm-usage.md` |
| リリース運用(サイクル・手順・配布物) | `docs/release.md` |
| JSON 出力の契約(check / stats / dump 系 / verify-move) | `docs/schema/`(`report`・`stats`・`dump-doc`・`dump-morph`・`dump-corpus`・`verify-move` の各 schema。検証テストは `crates/jaio-cli/tests/schema.rs`) |
| エージェント運用・必須コマンド・規約 | `../AGENTS.md` |
| 未解決事項・検証待ち | GitHub Issues(`gh issue list -R oboro-app/oboro-ja`) |

## 規約

- 本リポジトリのドキュメントは「丁寧(ですます体)かつ無駄のない文章」で書きます(設計書 §1.3)
- 設計判断を変える変更は、実装前に `docs/design.md` を更新します
- 作業進捗・実装済み/未実装の一覧は `docs/` に置きません。未解決事項は GitHub Issues で管理します
- ADR(`docs/decisions/`)は判断の記録が必要になった段階で分離可能。それまでは `design.md` に反映
- ログファイルはリポジトリ TOP の `logs/` に置きます(git 管理外)

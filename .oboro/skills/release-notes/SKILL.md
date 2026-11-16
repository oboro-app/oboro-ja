---
name: release-notes
description: Use when writing the release notes of a version, before publishing a release, so the notes list only what was done since the previous version, plainly and briefly.
---

# Release notes

Carried into the harness oboro generates for each repo. State only what this version changed since the previous tag. Plain and short.

## Rules

| Item | Rule |
|---|---|
| Content | Only what was done between the previous tag and this version |
| Never write | Descriptions of current features, requirements, install steps, usage, cautions, calls to action, explanatory prose, commit or PR titles and number lists, authors |
| Sections | Use only the ones with entries, in the repo's language. Japanese repos: "新規機能", "出力の変更", "解決した問題", "更新" (dependency or support-range updates) |
| Item | One line, a short noun phrase or plain form: "...を追加", "...を変更", "...を修正". No polite form |
| First version | Exactly "## 初回リリース" (the repo's language) and one item line, e.g. "- クローズドベータ版として公開" |
| Material | PRs and Issues merged since the previous tag that users can see. Skip tests, harness, docs, refactors, CI |
| Words | Use the words this repo's users use (command names, rule IDs, output terms). No internal words (daemon, ledger, worktree). Write the changed behavior, not vague words like "various improvements". Write only facts you verified landed in this version |
| Review | Have a different lineage than the writer check brevity, accuracy (really done in this version), omissions, and natural wording before publishing |

## Example

```
## 新規機能
- jaio dump-doc で文書構造の JSON を出力するコマンドを追加

## 解決した問題
- CRLF の文書で行番号が 1 つずれる問題を修正
```

## Flow

```mermaid
flowchart LR
  A[Merged PRs and Issues since the tag] --> B[Drop what users cannot see]
  B --> C[Sort into sections]
  C --> D[One line each]
  D --> E[Independent review]
  E --> F[Pass to the release command's notes]
```

This repo's release script (`scripts/release.sh`, driven by `docs/release.md`) composes the notes text itself; check its `--notes` body against these rules before publishing.

# Docs

The reader looks up one fact later, often an agent starting cold. From the heading and the first sentence they want to know whether this section answers their question.

Repo-specific: where each fact goes, what docs must not hold, and the doc reading order are in the [docs map](../../../../docs/README.md).

## Shape

```text
## <答える問いが分かる見出し>

<この節が何を決めているか。1-3 文>

| <同じ種類の物> | <性質> |
|---|---|
```

| Rule | Why |
|---|---|
| One row, one thing | A cell holding several rules cannot be found or linked |
| One fact in one place; link elsewhere | Two copies drift apart |
| The present state only | History and reasons live in commits and Issues |
| A level that survives a rewrite of the code | File paths, function names and pixel values go stale |

## Bad and good

Bad: one cell of a real term table, shortened. It holds five definitions, two rules, a color note and a design decision.

```text
| 「今」の 1 行の語 | 今 (どの画面でも消えないヘッダー下の 1 行。Progress の詳細の節「今」は Issue 単位で、こちらは画面全体の 1 行)、次 (目標の次に起きること。Issue 単位の次の動きとは別)、変化 (前に見た時からの数の増減)、予算待ち (...)、待つ物なし (...)。判断待ち、更新待ちは既存の語のまま使う。数は 99+ で止め、正確な値は説明で読む。色を使わず形で分ける。 |
```

Good: one word per row. The rules about numbers and color move to their own rows.

```text
| 語 | 意味 |
|---|---|
| 今 | 画面全体の状況を示す、ヘッダーの下の 1 行。どの画面でも消えない |
| 次 | 目標で次に起きること |
| 変化 | 前に見た時からの数の増減 |
```

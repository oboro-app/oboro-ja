---
name: writing-ja
description: Use when writing or revising Japanese text a human reads, for sentence length, subject-predicate distance, kanji-kana balance, redundancy, register, technical terms and katakana, this repo's coined words and their replacements, and symbol spacing.
---

# Writing Japanese

Write on top of the [writing](../writing/SKILL.md) skill: the scene, the title rules, the word rules and the AI slop table are there. This file adds what Japanese sentences need, and this repo's table of coined words.

## Sentences

These rules are for sentences in a body. An Issue title, a PR title and a commit subject follow "Titles" in the [writing skill](../writing/SKILL.md#titles). A heading or a label keeps the shape its scene gives it; when it runs long, cut words, and do not expand it into clauses.

| Rule | How | Example |
|---|---|---|
| One sentence, one point, under about 60 characters | Split at a claim change or around 50 to 60 characters | 「daemon を再起動する。その時、保存した設定を読み直す。」 |
| Keep subject and predicate close | Move the subject next to its verb; do not change the subject mid-sentence | 「Watcher は、利用状況を読み、段階を決める。」 |
| Commas mark dependency breaks | Put a comma where a wrong grouping changes the meaning | 「時間をかけて直した画面を閉じる。」は、読点の位置で「時間をかけた」のかかる先が変わる |
| Do not chain the same particle | Rewrite when 「の」 or 「に」 repeats in a row | 「監視の結果の通知」ではなく「監視した結果の通知」 |
| One predicate per clause | Do not stack 「と」「ため」「のに」 clauses; split into two sentences | 「A を B で止めると C と返って止まらない」ではなく「A を B で止められない。C と返る。」 |

## Word choice

| Rule | Example |
|---|---|
| Write kana where kana is standard | 出来る -> できる (「出来が良い」などは漢字)。有る、無い -> ある、ない (「問題がある」「欠点がない」)。概ね -> おおむね。語の表記を用語集や既存の文書が決めている時はそちらに従う |
| Connectives are not blanket kana | 接続詞の「さらに」は仮名で、副詞の「更に」「更なる」は漢字。「及び」「又は」は漢字が基本で、解説や広報では「および」「または」も使える |
| Cut 「すること」 and 「という」 fillers | 「確認することができます」 -> 「確認できます」。「agent というものは」 -> 「agent は」 |
| Break a noun chain in a sentence with a verb | 「Usage 監視結果通知機能」 -> 「Usage を監視した結果を通知する機能」 |
| Choose plain words over stiff office style | 「可及的速やかに」 -> 「できるだけ早く」。「喫緊の課題」 -> 「今すぐ直すべき課題」 |

## Register

| Text | Register |
|---|---|
| Documents under `docs/`, comments, commits, reports to humans | 常体 (である調)。会話の人格 (persona) を持ち込まない |
| Chat with the user, a report given in chat included | 敬体。ただし人間が別の口調を指示した時はその指示に従う |
| A Decision, and a question sent with `obr ask` | 敬体。daemon が作る Decision の文に合わせる |
| One document | One register throughout; do not mix 常体 and 敬体 |

## Technical terms and katakana

The word rules are "Words" in the [writing skill](../writing/SKILL.md#words). In Japanese:

| Case | Write |
|---|---|
| A term typed or shown as it is: a command, an identifier, a CLI, service or screen name | ASCII, as its owner spells it: `obr agent stop`、Usage、Decisions |
| A technical term: the list in "Words" of the writing skill, and terms like them | The English spelling in ASCII: agent, session, reviewer, worktree, daemon。エージェント、セッションとは書かない |
| An everyday word with a settled katakana form | Either spelling: ファイル or file、テスト or test。It is not a coined word. Pick one per text and keep it |
| Existing text that spells a technical term in katakana | Leave it unless you are revising that text. No bulk replacement |
| A term the reader may not know | Add a clause after it: 「rate limit (一定の時間に使える量の上限)」。Do not rename it |
| Unknown katakana on unknown katakana | Explain one of them in a plain clause |

An everyday technical word list for this repo: keep commands, rule IDs (`relation/broken-link` 等) and crate names in ASCII as they are.

## Coined words and their replacements

A coined word is a Japanese word standing where the reader already knows a term (a second name for a thing that has a name, a translated standard term, or a made-up noun for a state). Each row is checked against real text before being added. "Keep" lists ordinary uses of the same word; they stay.

No coined words are recorded in this repo yet. When one is found, add it with its replacement in the same PR.

## Symbols and spacing

| Rule | Example |
|---|---|
| Symbols are ASCII except 、 。 and 「」 | Use ASCII `-`, `:`, `()`. Do not use other full-width symbols |
| One space between half-width and full-width characters | 「3 つの窓」「70% から 90%」。詰めないことも開けすぎることもしない |

## Examples

Bad and fixed pairs, abstracted from real project writing.

| Hard to read | Easier to read |
|---|---|
| 本修正により、セッションの再開処理の安定性の向上が図られたものと考えられる。 | この修正で、止まっていた session を再開できるようになった。 |
| Usage が少なくなった場合におきましても、動作中のエージェントの停止が行われることはありません。 | Usage が少なくなっても、動いている agent は止まりません。 |
| 設定ファイルの読み込みの失敗時のリトライの実装を行うこと。 | 設定ファイルを読み込めなかった時は、読み直す。 |

## Sources

| Source | Used for | Checked |
|---|---|---|
| [「公用文作成の考え方」](https://www.bunka.go.jp/seisaku/bunkashingikai/kokugo/hokoku/93650001_01.html) (文化審議会建議、令和 4 年 1 月 7 日) | Sentence length, one point per sentence, subject-predicate distance, particle use, lists of three or more, kana choices | 2026-11-06 |

数字の字数目安 (50 から 60 字) は上記建議の解説にある目安である。実測の検証はしていない。

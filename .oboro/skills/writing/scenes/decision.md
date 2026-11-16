# Decision

The reader is a human with no context, deciding alone, perhaps hours later. The five parts and their order are in the [writing skill](../SKILL.md#decisions-and-questions-to-a-human).

| Rule | Why |
|---|---|
| "What happened" is the fact, in words the human uses | A suspected cause sends the human the wrong way |
| Each option says what happens if chosen | A label alone cannot be weighed |
| "If left" says whether it resolves by itself, and when | The human decides from it whether to answer now |
| One line per option: the label, then what happens | Two options in one sentence cannot be compared |
| No coined words | The human cannot look them up |

## Bad and good

Bad: a real Decision before it was rewritten. A coined noun (届いていない報告), no reason a human is needed, and nothing on what happens if it is left.

```text
目標「X」を進めるメインエージェントがいません。届いていない報告: 2 件。Main を始めると、目標と届いていない報告を引き継ぎます。
```

Good: the same Decision after the rewrite.

```text
何が起きたか: 目標「X」を進める Main の session が止まっています。Main に未読の message が 2 件あります。
なぜ人間が要るか: Main がいないと agent の報告を誰も受け取れず、目標が進みません。自動で再起動できなかったため、人間が始めます。
選択肢と結果:
- Start Main: 新しい Main を起動し、目標と未読の message を引き継ぎます。
判断しないとどうなるか: Start Main を押すまで、この目標は進みません。自動では解決しません。
詳しく読むには: 目標の状態は Goal の画面、止まった session の経緯は log で読めます。
```

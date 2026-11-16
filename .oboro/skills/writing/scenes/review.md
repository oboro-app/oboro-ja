# Review comment

The owner must fix what you found. They want the verdict, then the worst finding, each with enough to reproduce it. They do not need the PR described back to them.

## Shape

```text
<pass か changes>。<changes は一番重い指摘を 1 文で。pass は確かめた範囲を 1 文で>

1. [P1] <何が壊れるか> (<file>:<line>)
   <どの入力や操作で何が起きるか。確かめた command と結果>
   <直った状態>
```

| Rule | Why |
|---|---|
| The verdict is the first word | The gate and the owner both read it first |
| Each finding has a place, what breaks, and the wanted state | "気になります" cannot be fixed |
| A concrete input and the wrong result | The owner turns it into a failing test |
| A severity on each finding, worst first | The owner knows what blocks the merge |
| No summary of the PR, no praise | The owner wrote the PR |

## Bad and good

Bad: the opening of a real review. One sentence of 130 characters retells the design, and the verdict comes last.

```text
設計と本体の実装は妥当。既存の Worker/D1/非公開 R2 の上に載せて schema 変更なく、昇格は同一配布物を割合 0 で写して許可の印を監査に残し、switch=1 の時だけ古い版を返し割合と最低の版の関門はそのまま、semver の順序は updater と同じ仕様どおり、beta は既定の経路として読める。ただし最新 commit で repo の検査が落ちるため直しを求める。
```

Good: the same opening.

```text
changes。最新の commit で `check_docs.py` と `check_scenarios.py` が落ちる。設計は Issue の目的に合っている。
```

Good: a real finding. The place, a concrete input, the wrong result and the wanted state are all there.

```text
1. [P1] stable の直接登録で、昇格の許可を迂回できる (services/release-api/src/admin.ts:66)
   POST /admin/releases は channel=stable を無条件で受け付ける。nightly にない 0.99.0 を approval なし、rollout=100 で登録すると 201 が返り、stable の端末への確認も 200 になった。
   stable への書き込みを、承認済みの昇格の経路だけにする。
```

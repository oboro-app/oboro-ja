//! `docs/schema/` の JSON Schema 本文。ビルド時に埋め込む。

/// `jaio check --format json` の契約
pub const REPORT: &str = include_str!("../../../docs/schema/report.schema.json");

/// `jaio stats --format json` の契約
pub const STATS: &str = include_str!("../../../docs/schema/stats.schema.json");

/// `jaio verify-move --format json` の契約
pub const VERIFY_MOVE: &str = include_str!("../../../docs/schema/verify-move.schema.json");

/// `jaio dump doc` の契約
pub const DUMP_DOC: &str = include_str!("../../../docs/schema/dump-doc.schema.json");

/// `jaio dump morph` の契約
pub const DUMP_MORPH: &str = include_str!("../../../docs/schema/dump-morph.schema.json");

/// `jaio dump corpus` の契約
pub const DUMP_CORPUS: &str = include_str!("../../../docs/schema/dump-corpus.schema.json");

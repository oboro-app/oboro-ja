//! トークン計測(tiktoken)。トークン効率の実測に使う(設計書 §5.1)。

use anyhow::Result;
use jaio_core::config::TokenEncoding;
use tiktoken_rs::CoreBPE;

/// BPE トークンカウンタ。
pub struct TokenCounter {
    bpe: CoreBPE,
}

impl TokenCounter {
    /// o200k_base エンコーディングで作る。
    pub fn new() -> Result<Self> {
        Self::with_encoding(TokenEncoding::default())
    }

    /// エンコーディングを指定して作る(設計書 §8 `[analysis] encoding`)。
    pub fn with_encoding(encoding: TokenEncoding) -> Result<Self> {
        let bpe = match encoding {
            TokenEncoding::O200kBase => tiktoken_rs::o200k_base()?,
            TokenEncoding::Cl100kBase => tiktoken_rs::cl100k_base()?,
        };
        Ok(Self { bpe })
    }

    /// テキストのトークン数を数える。
    pub fn count(&self, text: &str) -> usize {
        self.bpe.encode_ordinary(text).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_tokens() -> Result<()> {
        let counter = TokenCounter::new()?;
        // トークン数は空文字列で 0、非空で正の値
        assert_eq!(counter.count(""), 0);
        assert!(counter.count("日本語のトークン数を計測する。") > 0);
        Ok(())
    }
}

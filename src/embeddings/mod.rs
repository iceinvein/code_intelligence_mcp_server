pub mod candle;
pub mod hash;
#[cfg(feature = "model-download")]
pub mod model_download;

use anyhow::Result;

pub trait Embedder {
    fn dim(&self) -> usize;
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

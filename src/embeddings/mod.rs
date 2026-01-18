pub mod fastembed;
pub mod hash;

use anyhow::Result;

pub trait Embedder {
    fn dim(&self) -> usize;
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

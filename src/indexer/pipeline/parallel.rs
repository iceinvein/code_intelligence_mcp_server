// This module is deprecated in favor of the unified parse → write → embed pipeline.
// The old parallel indexing code has been replaced by:
// - parse::parse_files() for parallel parsing
// - write::write_batch() for batched writes
// - IndexPipeline::generate_embeddings_for_orphaned_symbols() for embeddings
//
// This file is kept for backwards compatibility but contains no active code.
